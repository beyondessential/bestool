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

use bestool_canopy::{
	TargetOutcome,
	schema::{BackupPurpose, ReportArgs, RunOutcome},
};
use bestool_kopia::{
	RunAs, S3KopiaEnv, Snapshot, args_snapshot_list, args_snapshot_restore,
	build_kopia_command_with_s3, find_kopia_binary,
};
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result, bail, miette};
use tracing::{info, warn};
use uuid::Uuid;

use super::backup::{
	base_url_of, build_client, config, connect_repo, load_registration, method::RestoreOpts,
	run_kopia, run_kopia_visible, spawn_proxy, transient_config_dir, trim_error,
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
	let config_dir = transient_config_dir()?;
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
	let snapshot = select_snapshot(&snapshots, &args.id)?;
	info!(
		id = %snapshot.id,
		taken = ?snapshot.end_time.or(snapshot.start_time),
		"restoring snapshot",
	);

	// A fresh run id per restore (canopy rejects a repeated one).
	let run_id = Uuid::new_v4();

	// Perform the restore, capturing the outcome so it can be reported to canopy
	// whether it succeeds or fails.
	let outcome = run_restore(&kopia, &s3env, snapshot, &def, &args).await;

	// Report to canopy so the restore shows up in the fleet table. The restore's
	// own outcome is what the command returns; a reporting failure is only warned.
	let traffic = proxy.traffic();
	let to_i64 = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
	let report = ReportArgs::builder()
		.run_id(run_id)
		.type_(args.backup_type.clone())
		.purpose(BackupPurpose::Restore)
		.outcome(if outcome.is_ok() {
			RunOutcome::Success
		} else {
			RunOutcome::Failure
		})
		.maybe_error(outcome.as_ref().err().map(trim_error))
		.snapshot_id(snapshot.id.clone())
		.s3_sent_raw_bytes(to_i64(traffic.sent_raw))
		.s3_sent_payload_bytes(to_i64(traffic.sent_payload))
		.s3_received_raw_bytes(to_i64(traffic.received_raw))
		.s3_received_payload_bytes(to_i64(traffic.received_payload))
		.build();
	if let Err(err) = client.backup_report(&report).await {
		warn!("failed to report the restore to canopy: {err}");
	}

	outcome
}

/// Run the kopia restore into a staging dir and lay it down via the def's method.
async fn run_restore(
	kopia: &std::path::Path,
	s3env: &S3KopiaEnv<'_>,
	snapshot: &Snapshot,
	def: &config::BackupDef,
	args: &RestoreArgs,
) -> Result<()> {
	// Restore into a staging dir colocated with the target's filesystem.
	let staging = def
		.method
		.staging_dir(args.target.as_deref(), std::process::id());
	if staging.exists() {
		tokio::fs::remove_dir_all(&staging).await.ok();
	}

	// The download needs room for the whole snapshot on the staging volume. Check
	// up front and, if short, let the operator free space and retry rather than
	// fail deep into the download.
	ensure_free_space(&staging, snapshot.total_size()).await?;

	let mut restore_cmd = build_kopia_command_with_s3(kopia, s3env, RunAs::CurrentUser)
		.map_err(|e| miette!("{e}"))?;
	// Force kopia's progress display (a large restore is otherwise a silent wait)
	// and run it against the inherited terminal so it's actually visible.
	restore_cmd.arg("--progress");
	args_snapshot_restore(&mut restore_cmd, &snapshot.id, &staging);
	run_kopia_visible(restore_cmd, "snapshot restore").await?;

	let clobber = args.clobber || confirm_clobber_interactively(&args.backup_type)?;
	let opts = RestoreOpts {
		target: args.target.clone(),
		clobber,
	};
	def.method.restore(&staging, &opts).await
}

/// Error unless the volume backing `staging` has room for `needed` bytes (plus a
/// little headroom), prompting the operator to free space and retry on an
/// interactive terminal. Skipped when the snapshot size is unknown.
async fn ensure_free_space(staging: &std::path::Path, needed: Option<i64>) -> Result<()> {
	let Some(needed) = needed.filter(|n| *n > 0).map(|n| n as u64) else {
		return Ok(()); // unknown size (no root summary): nothing to check against
	};
	// 5% headroom for filesystem overhead and rounding. The swap into place is a
	// rename (the old data is kept as `.old` in place), so only the staging copy
	// consumes new space.
	let required = needed.saturating_add(needed / 20);
	// Check the parent: `staging` itself doesn't exist yet.
	let volume = staging.parent().unwrap_or(staging).to_path_buf();
	crate::interactive::retry("ensuring enough free disk space", async || {
		let available = fs4::available_space(&volume)
			.into_diagnostic()
			.wrap_err_with(|| format!("checking free space on {}", volume.display()))?;
		if available >= required {
			return Ok(());
		}
		bail!(
			"restoring needs about {} free on {} but only {} is available; free up space and retry",
			human_bytes(required),
			volume.display(),
			human_bytes(available),
		)
	})
	.await
}

/// A rough human-readable byte size (binary units), for operator-facing messages.
fn human_bytes(bytes: u64) -> String {
	const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
	let mut value = bytes as f64;
	let mut unit = 0;
	while value >= 1024.0 && unit < UNITS.len() - 1 {
		value /= 1024.0;
		unit += 1;
	}
	if unit == 0 {
		format!("{bytes} B")
	} else {
		format!("{value:.1} {}", UNITS[unit])
	}
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
/// Selection is by snapshot id alone. It is deliberately not scoped to the
/// local server — a restore is typically onto a different (rebuilt or
/// replacement) host, so the snapshot's source host is not this server's id —
/// nor gated on the `canopy-type` tag: kopia's `snapshot list` does not echo
/// the tags set at create time, so every listed snapshot deserialises with no
/// tags and gating on them would reject every snapshot. The backup type still
/// selects the def, method, and credentials in the caller.
fn select_snapshot<'a>(snapshots: &'a [Snapshot], id: &str) -> Result<&'a Snapshot> {
	let mut hits = snapshots.iter().filter(|s| s.id.starts_with(id));
	let Some(first) = hits.next() else {
		bail!(
			"no snapshot matching id '{id}' in the repository{}",
			available_snapshots_hint(snapshots)
		);
	};
	if hits.next().is_some() {
		bail!("snapshot id '{id}' is ambiguous; give more characters");
	}
	Ok(first)
}

/// Describe what the connected repository actually holds, appended to the
/// "no match" error so the operator can see the ids available rather than
/// guess. Newest first, capped so the message stays readable.
fn available_snapshots_hint(snapshots: &[Snapshot]) -> String {
	if snapshots.is_empty() {
		return "; the repository has no snapshots".to_owned();
	}
	const MAX: usize = 20;
	let mut sorted: Vec<&Snapshot> = snapshots.iter().collect();
	sorted.sort_by_key(|s| std::cmp::Reverse(s.end_time.or(s.start_time)));
	let mut out = format!("; {} snapshot(s) available (id, source, taken):", snapshots.len());
	for s in sorted.iter().take(MAX) {
		let taken = s
			.end_time
			.or(s.start_time)
			.map_or_else(|| "unknown".to_owned(), |t| t.to_string());
		out.push_str(&format!("\n  {} {} {taken}", s.id, s.source.host));
	}
	if snapshots.len() > MAX {
		out.push_str(&format!("\n  … and {} more", snapshots.len() - MAX));
	}
	out
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

	/// A snapshot as kopia's `snapshot list --json` actually emits it: source
	/// host and id, but no tags (kopia does not echo the create-time tags).
	fn snap(id: &str, host: &str, end: Option<&str>) -> Snapshot {
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
			tags: std::collections::BTreeMap::new(),
			root_entry: None,
		}
	}

	#[test]
	fn selects_by_id_prefix() {
		let snaps = vec![snap("abc123", "srv", None)];
		let chosen = select_snapshot(&snaps, "abc").unwrap();
		assert_eq!(chosen.id, "abc123");
	}

	#[test]
	fn selects_when_kopia_omits_tags() {
		// The regression: kopia's snapshot list returns no tags, so gating on the
		// canopy-type tag rejected every snapshot. Selection by id must still find
		// a valid, correct id.
		let snaps = vec![snap("99f1f3f6e25f483b5196d61d2f28a871", "srv", None)];
		let chosen = select_snapshot(&snaps, "99f1f3f6e25f483b5196d61d2f28a871").unwrap();
		assert_eq!(chosen.id, "99f1f3f6e25f483b5196d61d2f28a871");
	}

	#[test]
	fn selects_across_hosts() {
		// A restore onto a different host: the snapshot's source host is not this
		// server's id, but selection by id must still find it.
		let snaps = vec![snap("abc123", "other-host", None)];
		let chosen = select_snapshot(&snaps, "abc").unwrap();
		assert_eq!(chosen.id, "abc123");
	}

	#[test]
	fn errors_on_ambiguous_prefix() {
		let snaps = vec![snap("abc123", "srv", None), snap("abc456", "other", None)];
		assert!(select_snapshot(&snaps, "abc").is_err());
	}

	#[test]
	fn errors_when_no_match_lists_available() {
		let snaps = vec![snap("abc", "srv", Some("2026-01-01T00:00:00Z"))];
		let err = select_snapshot(&snaps, "zzz").unwrap_err().to_string();
		assert!(err.contains("no snapshot matching id 'zzz'"));
		assert!(err.contains("abc"));
	}

	#[test]
	fn errors_when_repository_empty() {
		let snaps: Vec<Snapshot> = vec![];
		let err = select_snapshot(&snaps, "abc").unwrap_err().to_string();
		assert!(err.contains("no snapshots"));
	}

	#[test]
	fn human_bytes_scales_units() {
		assert_eq!(human_bytes(512), "512 B");
		assert_eq!(human_bytes(1024), "1.0 KiB");
		assert_eq!(human_bytes(8 * 1024 * 1024 * 1024), "8.0 GiB");
	}

	#[tokio::test]
	async fn ensure_free_space_skips_unknown_size() {
		// No snapshot size: nothing to check, so it never touches the filesystem.
		ensure_free_space(std::path::Path::new("/nonexistent/staging"), None)
			.await
			.unwrap();
	}
}
