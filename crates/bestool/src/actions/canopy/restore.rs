//! `bestool canopy restore`: restore a backup, method-aware.
//!
//! Reads the def for `--type`, fetches restore-purpose creds, kopia-restores the
//! selected snapshot into a staging dir, and dispatches to the method's restore
//! (the `postgresql` method does the full stop/swap/start). Refuses to overwrite
//! existing data unless `--clobber-existing-data-yes-i-am-sure` or an interactive
//! confirmation is given.

use std::{
	io::{IsTerminal as _, Write as _},
	path::PathBuf,
};

use bestool_canopy::{TargetOutcome, schema::BackupPurpose};
use bestool_kopia::{
	RunAs, S3KopiaEnv, Snapshot, args_snapshot_list, args_snapshot_restore,
	build_kopia_command_with_s3, find_kopia_binary,
};
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result, bail, miette};
use tracing::info;

use super::backup::{
	base_url_of, build_client, config, connect_repo, load_registration, method::RestoreOpts,
	run_kopia, spawn_proxy,
};
use crate::actions::Context;

/// Restore a backup from Canopy's repository.
#[derive(Debug, Clone, Parser)]
pub struct RestoreArgs {
	/// The backup type to restore (must have a def in the backups directory).
	#[arg(value_name = "TYPE")]
	pub backup_type: String,

	/// The snapshot id to restore (a prefix is accepted).
	#[arg(value_name = "ID")]
	pub id: String,

	/// Override the destination (the simple method's path); postgresql always
	/// targets its configured cluster.
	#[arg(long, value_name = "PATH")]
	pub target: Option<PathBuf>,

	/// Proceed even if the destination already contains data (non-interactive).
	#[arg(long = "clobber-existing-data-yes-i-am-sure")]
	pub clobber: bool,

	/// Override the registration directory.
	#[arg(long, value_name = "DIR")]
	pub config: Option<PathBuf>,

	/// Override the backups definition directory.
	#[arg(long, value_name = "DIR")]
	pub backups_dir: Option<PathBuf>,
}

pub async fn run(args: RestoreArgs, _ctx: Context) -> Result<()> {
	let dir = args
		.backups_dir
		.clone()
		.unwrap_or_else(config::backups_dir);
	let def = config::find_def(&dir, &args.backup_type)
		.await?
		.ok_or_else(|| {
			miette!(
				"no backup def for type '{}' in {}",
				args.backup_type,
				dir.display()
			)
		})?;

	let reg = load_registration(args.config.as_deref())
		.await?
		.ok_or_else(|| miette!("not registered with canopy; run `bestool canopy register` first"))?;
	let device_key = reg
		.device_key
		.clone()
		.ok_or_else(|| miette!("registration has no device key"))?;
	let server_id = reg
		.server_id
		.clone()
		.ok_or_else(|| miette!("registration has no server id"))?;
	let client = build_client(base_url_of(&reg)?, &device_key).await?;

	let target = match TargetOutcome::from_result(client.backup_target().await)? {
		TargetOutcome::Ready(target) => target,
		TargetOutcome::Dormant => {
			bail!("device is not authorised for this backup repository (cannot restore)")
		}
	};

	// Read-only creds + connection (the restore purpose downscopes server-side).
	// The proxy serves for the whole restore; held in scope to the end.
	let proxy = spawn_proxy(
		client.clone(),
		args.backup_type.clone(),
		BackupPurpose::Restore,
		&target.region,
	)
	.await?;
	let config_dir = tempfile::tempdir()
		.into_diagnostic()
		.wrap_err("creating transient kopia config dir")?;
	let config_path = config_dir.path().join("repository.config");
	let s3env = S3KopiaEnv {
		password: &target.repo_password.0,
		config_path: &config_path,
	};
	let kopia = find_kopia_binary(None).ok_or_else(|| miette!("could not find the kopia binary"))?;
	connect_repo(
		&kopia,
		&s3env,
		&target,
		&proxy.endpoint(),
		&server_id,
		RunAs::CurrentUser,
	)
	.await?;

	// Select the snapshot to restore.
	let snapshots = list_snapshots(&kopia, &s3env).await?;
	let snapshot = select_snapshot(&snapshots, &args.backup_type, &args.id)?;
	info!(
		id = %snapshot.id,
		taken = ?snapshot.end_time.or(snapshot.start_time),
		"restoring snapshot",
	);

	// Restore into a staging dir colocated with the target's filesystem.
	let staging = def
		.method
		.staging_dir(args.target.as_deref(), std::process::id());
	if staging.exists() {
		tokio::fs::remove_dir_all(&staging).await.ok();
	}
	let mut restore_cmd = build_kopia_command_with_s3(&kopia, &s3env, RunAs::CurrentUser)
		.map_err(|e| miette!("{e}"))?;
	args_snapshot_restore(&mut restore_cmd, &snapshot.id, &staging);
	run_kopia(restore_cmd, "snapshot restore").await?;

	let clobber = args.clobber || confirm_clobber_interactively(&args.backup_type)?;
	let opts = RestoreOpts {
		target: args.target.clone(),
		clobber,
	};
	def.method.restore(&staging, &opts).await
}

async fn list_snapshots(kopia: &std::path::Path, s3env: &S3KopiaEnv<'_>) -> Result<Vec<Snapshot>> {
	let mut cmd = build_kopia_command_with_s3(kopia, s3env, RunAs::CurrentUser)
		.map_err(|e| miette!("{e}"))?;
	args_snapshot_list(&mut cmd);
	let stdout = run_kopia(cmd, "snapshot list").await?;
	serde_json::from_str(stdout.trim())
		.into_diagnostic()
		.wrap_err("parsing kopia snapshot list")
}

/// Pick the snapshot to restore from the repo's list by id prefix.
///
/// Selection is deliberately not scoped to the local server: a restore is
/// typically onto a different (rebuilt or replacement) host than the one that
/// took the backup, so the snapshot's source host is not this server's id.
fn select_snapshot<'a>(
	snapshots: &'a [Snapshot],
	backup_type: &str,
	id: &str,
) -> Result<&'a Snapshot> {
	let mut hits = snapshots.iter().filter(|s| {
		s.id.starts_with(id)
			&& s.tags.get("canopy-type").map(String::as_str) == Some(backup_type)
	});
	let first = hits
		.next()
		.ok_or_else(|| miette!("no '{backup_type}' snapshot matching id '{id}' in the repository"))?;
	if hits.next().is_some() {
		bail!("snapshot id '{id}' is ambiguous; give more characters");
	}
	Ok(first)
}

/// Interactive double-confirmation for a destructive restore. Returns `true`
/// only when both prompts pass. With no TTY, returns `false` (the caller then
/// relies on the explicit flag / the clobber guard).
fn confirm_clobber_interactively(backup_type: &str) -> Result<bool> {
	if !std::io::stdin().is_terminal() {
		return Ok(false);
	}
	print!(
		"This will OVERWRITE existing data for '{backup_type}'. Continue? [y/N] "
	);
	std::io::stdout().flush().ok();
	if !read_line()?.trim().eq_ignore_ascii_case("y") {
		return Ok(false);
	}
	print!("Type the backup type '{backup_type}' to confirm: ");
	std::io::stdout().flush().ok();
	Ok(read_line()?.trim() == backup_type)
}

fn read_line() -> Result<String> {
	let mut buf = String::new();
	std::io::stdin()
		.read_line(&mut buf)
		.into_diagnostic()
		.wrap_err("reading confirmation")?;
	Ok(buf)
}

#[cfg(test)]
mod tests {
	use bestool_kopia::SnapshotSource;

	use super::*;

	fn snap(id: &str, host: &str, btype: &str, end: Option<&str>) -> Snapshot {
		Snapshot {
			id: id.into(),
			source: SnapshotSource {
				host: host.into(),
				user_name: "canopy".into(),
				path: "/x".into(),
			},
			description: String::new(),
			start_time: None,
			end_time: end.map(|t| t.parse().unwrap()),
			tags: std::collections::BTreeMap::from([("canopy-type".into(), btype.into())]),
			root_entry: None,
		}
	}

	#[test]
	fn selects_by_id_prefix() {
		let snaps = vec![snap("abc123", "srv", "tamanu-postgres", None)];
		let chosen = select_snapshot(&snaps, "tamanu-postgres", "abc").unwrap();
		assert_eq!(chosen.id, "abc123");
	}

	#[test]
	fn selects_across_hosts() {
		// A restore onto a different host: the snapshot's source host is not this
		// server's id, but selection by id must still find it.
		let snaps = vec![snap("abc123", "other-host", "tamanu-postgres", None)];
		let chosen = select_snapshot(&snaps, "tamanu-postgres", "abc").unwrap();
		assert_eq!(chosen.id, "abc123");
	}

	#[test]
	fn errors_on_ambiguous_prefix() {
		let snaps = vec![
			snap("abc123", "srv", "tamanu-postgres", None),
			snap("abc456", "other", "tamanu-postgres", None),
		];
		assert!(select_snapshot(&snaps, "tamanu-postgres", "abc").is_err());
	}

	#[test]
	fn errors_when_no_match() {
		let snaps = vec![snap("abc", "srv", "tamanu-postgres", None)];
		assert!(select_snapshot(&snaps, "tamanu-postgres", "zzz").is_err());
	}

	#[test]
	fn errors_on_type_mismatch() {
		let snaps = vec![snap("abc123", "srv", "files", None)];
		assert!(select_snapshot(&snaps, "tamanu-postgres", "abc").is_err());
	}
}
