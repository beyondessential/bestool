//! `pg_basebackup` base backup.
//!
//! For backends with no cheap atomic snapshot (ext4/xfs, thick LVM):
//! `pg_basebackup --wal-method=stream` streams a complete base backup with the
//! WAL **and the backup-end record** bundled in, so it restores by clean crash
//! recovery — no `pg_resetwal`, no forced REINDEX. Always correct, just heavier
//! than a CoW snapshot (a full copy each run).
//!
//! The streaming + chown steps are privileged and verified on-host; the pure
//! helpers (paths, argv) are unit-tested.

use std::{
	path::{Path, PathBuf},
	process::Stdio,
};

use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use tracing::info;

use super::{super::method::PostgresqlConfig, resolve::ResolvedCluster};

/// Where this run's base backup is streamed to (and what kopia snapshots). Nests
/// `<version>/<cluster>` under the chosen staging root, matching the btrfs
/// strategy's layout so the kopia source path lines up.
fn destination(root: &Path, resolved: &ResolvedCluster) -> PathBuf {
	root.join(&resolved.version).join(&resolved.cluster)
}

/// `pg_basebackup` args for a plain (uncompressed) base backup with streamed WAL.
/// Connection params (`connection_url` or socket/port) are applied separately by
/// [`super::apply_connection`]. `--no-password` keeps it from ever prompting.
fn basebackup_args(dest: &Path) -> Vec<String> {
	vec![
		"-D".to_owned(),
		dest.to_string_lossy().into_owned(),
		"--wal-method=stream".to_owned(),
		"--checkpoint=fast".to_owned(),
		"--no-password".to_owned(),
	]
}

/// Stream a base backup and return (kopia source path, the dir to clean up).
pub async fn prepare(
	resolved: &ResolvedCluster,
	backup_type: &str,
	config: &PostgresqlConfig,
	need: Option<u64>,
) -> Result<(PathBuf, PathBuf)> {
	// Pick a staging root with room for the full copy (a roomier disk when the
	// default is too small), failing early if nothing fits.
	let root = super::space::choose_staging_root(backup_type, config.staging_dir.as_deref(), need)?;
	let dest = destination(&root, resolved);

	// pg_basebackup requires an empty target; clear leftovers from a crashed run.
	if root.exists() {
		remove_staging(&root).await?;
	}
	if let Some(parent) = dest.parent() {
		tokio::fs::create_dir_all(parent)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("creating {}", parent.display()))?;
	}

	// pg_basebackup runs as the postgres user (peer auth) and creates its own
	// output dir, so the root-owned staging must be writable by postgres first.
	#[cfg(unix)]
	make_writable_by_postgres(&root).await?;

	info!(dest = %dest.display(), "streaming pg_basebackup");
	let mut cmd = super::pg_command(&super::postgres_bin("pg_basebackup", &resolved.data_dir));
	cmd.args(basebackup_args(&dest));
	super::apply_connection(&mut cmd, config);
	cmd.stdin(Stdio::null());
	// Capture stderr: it carries the operative error (auth, WAL config, disk), and
	// with inherited stderr all that reaches canopy is the exit status.
	let output = cmd
		.output()
		.await
		.into_diagnostic()
		.wrap_err("spawning pg_basebackup")?;
	let stderr = String::from_utf8_lossy(&output.stderr);
	if !output.status.success() {
		bail!("{}", failure_message(output.status, &stderr));
	}
	if !stderr.trim().is_empty() {
		info!(stderr = %stderr.trim(), "pg_basebackup");
	}

	#[cfg(unix)]
	make_readable_by_kopia(&root).await;
	Ok((dest, root))
}

/// The error for a failed run: exit status plus what pg_basebackup said on
/// stderr, so the report to canopy carries the actual cause.
fn failure_message(status: impl std::fmt::Display, stderr: &str) -> String {
	let stderr = stderr.trim();
	if stderr.is_empty() {
		format!("pg_basebackup failed ({status})")
	} else {
		format!("pg_basebackup failed ({status}): {stderr}")
	}
}

/// Remove the streamed base backup (a full copy; reclaim the space).
pub async fn teardown(root: PathBuf) -> Result<()> {
	remove_staging(&root).await
}

/// Delete a staging tree the daemon had handed to postgres/kopia. Reclaim
/// ownership first: root holds CAP_CHOWN but not DAC write-override, so it can't
/// unlink files inside postgres-/kopia-owned directories otherwise.
async fn remove_staging(root: &Path) -> Result<()> {
	#[cfg(unix)]
	{
		let _ = tokio::process::Command::new("chown")
			.args(["-R", "root:root"])
			.arg(root)
			.stdin(Stdio::null())
			.status()
			.await;
	}
	tokio::fs::remove_dir_all(root)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("removing base backup at {}", root.display()))
}

/// Hand the (root-created) staging to the postgres user so pg_basebackup, which
/// runs as postgres, can create its output dir and write into it. Required —
/// without it the stream fails to create its target.
#[cfg(unix)]
async fn make_writable_by_postgres(root: &Path) -> Result<()> {
	let status = tokio::process::Command::new("chown")
		.args(["-R", "postgres:postgres"])
		.arg(root)
		.stdin(Stdio::null())
		.status()
		.await
		.into_diagnostic()
		.wrap_err("handing the base backup staging to the postgres user")?;
	if !status.success() {
		bail!("chown of base backup staging to postgres failed ({status})");
	}
	Ok(())
}

/// pg_basebackup writes files owned by postgres and mode 0600; hand them to the
/// kopia user so the (elevated-to-kopia) kopia run can read them. Best-effort —
/// when there's no kopia user the run is direct (current user already reads it).
#[cfg(unix)]
async fn make_readable_by_kopia(root: &Path) {
	let kopia_exists = tokio::process::Command::new("id")
		.args(["-u", "kopia"])
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.status()
		.await
		.map(|s| s.success())
		.unwrap_or(false);
	if !kopia_exists {
		return;
	}
	let status = tokio::process::Command::new("chown")
		.args(["-R", "kopia:kopia"])
		.arg(root)
		.stdin(Stdio::null())
		.status()
		.await;
	if !matches!(status, Ok(s) if s.success()) {
		tracing::warn!("could not chown base backup to the kopia user; kopia may fail to read it");
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn destination_nests_version_and_cluster_under_root() {
		let resolved = ResolvedCluster {
			data_dir: "/var/lib/postgresql/16/main".into(),
			version: "16".into(),
			cluster: "main".into(),
		};
		assert_eq!(
			destination(Path::new("/staging/tamanu-postgres"), &resolved),
			Path::new("/staging/tamanu-postgres/16/main")
		);
	}

	#[test]
	fn failure_message_carries_stderr() {
		assert_eq!(
			failure_message(
				"exit code: 1",
				"pg_basebackup: error: connection to server at \"localhost\" failed: FATAL: role \"SYSTEM\" does not exist\n",
			),
			"pg_basebackup failed (exit code: 1): pg_basebackup: error: connection to server at \"localhost\" failed: FATAL: role \"SYSTEM\" does not exist"
		);
	}

	#[test]
	fn failure_message_without_stderr_is_just_the_status() {
		assert_eq!(
			failure_message("exit code: 1", "  \n"),
			"pg_basebackup failed (exit code: 1)"
		);
	}

	#[test]
	fn basebackup_args_stream_wal_and_fast_checkpoint() {
		// Connection params are applied separately (see `apply_connection`); these
		// are just the fixed base-backup flags.
		let args = basebackup_args(Path::new("/staging/16/main"));
		assert_eq!(
			args,
			vec![
				"-D",
				"/staging/16/main",
				"--wal-method=stream",
				"--checkpoint=fast",
				"--no-password",
			]
		);
	}
}
