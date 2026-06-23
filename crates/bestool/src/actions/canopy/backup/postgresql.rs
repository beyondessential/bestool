//! The `postgresql` backup method: physical, crash-consistent cluster snapshots.
//!
//! Generic postgres (no Tamanu coupling): driven by the `[postgresql]` config
//! table. Resolves the cluster's data directory, issues a best-effort
//! `CHECKPOINT` to bound WAL replay on restore, detects the storage backend, and
//! captures it: a crash-consistent btrfs or thin-LVM snapshot where available,
//! else a `pg_basebackup` base backup. (Windows VSS is the remaining backend.)

pub mod basebackup;
pub mod btrfs;
pub mod lvm;
pub mod resolve;
pub mod strategy;
mod sys;

use std::{
	collections::BTreeMap,
	path::{Path, PathBuf},
};

use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use tracing::{info, warn};

use self::strategy::Strategy;
use super::method::{PostgresqlConfig, Prepared, Teardown};

/// The stable path the snapshot/basebackup is exposed at for kopia — fixed per
/// backup type so kopia's history/dedup attribute to one source, regardless of
/// which strategy produced it (a host migrating btrfs↔basebackup keeps its
/// history). The version/cluster suffix the caller adds is the only moving part.
pub(super) fn stable_source_dir(backup_type: &str) -> PathBuf {
	PathBuf::from("/var/lib/kopia/bestool-backup").join(backup_type)
}

/// Transient files safe to exclude from the snapshot. Never `pg_wal`, `pg_xact`,
/// `pg_control`, `global`, or tablespaces — those are required for recovery.
fn ignore_globs() -> Vec<String> {
	["postmaster.pid", "*.log", "pg_stat_tmp/*", "lost+found"]
		.into_iter()
		.map(String::from)
		.collect()
}

/// Snapshot metadata carried as kopia tags (drives observability + restore).
fn metadata_tags(resolved: &resolve::ResolvedCluster, strategy: Strategy) -> BTreeMap<String, String> {
	BTreeMap::from([
		("pg-version".to_owned(), resolved.version.clone()),
		("pg-cluster".to_owned(), resolved.cluster.clone()),
		("pg-strategy".to_owned(), format!("{strategy:?}").to_lowercase()),
	])
}

/// Prepare a crash-consistent source for kopia.
pub async fn prepare(config: &PostgresqlConfig, backup_type: &str) -> Result<Prepared> {
	let resolved = resolve::resolve(config)?;
	let strategy = strategy::detect(config.strategy.as_deref(), &resolved.data_dir)?;
	info!(
		cluster = %resolved.cluster,
		version = %resolved.version,
		?strategy,
		data_dir = %resolved.data_dir.display(),
		"preparing postgresql backup",
	);

	// An explicit CHECKPOINT just before the snapshot bounds how much WAL
	// recovery replays on restore. It's an optimisation, not a correctness
	// requirement — the snapshot is crash-consistent regardless — so a failure
	// here must not fail the backup.
	checkpoint(config).await;

	match strategy {
		Strategy::Btrfs => {
			let (source, mounts) = btrfs::prepare(&resolved, backup_type).await?;
			Ok(Prepared {
				path: source,
				extra_tags: metadata_tags(&resolved, strategy),
				ignore: ignore_globs(),
				teardown: Teardown::Btrfs(mounts),
			})
		}
		Strategy::ThinLvm => {
			let (source, snapshot) = lvm::prepare(&resolved, backup_type).await?;
			Ok(Prepared {
				path: source,
				extra_tags: metadata_tags(&resolved, strategy),
				ignore: ignore_globs(),
				teardown: Teardown::Lvm(snapshot),
			})
		}
		Strategy::BaseBackup => {
			let (source, root) = basebackup::prepare(
				&resolved,
				backup_type,
				config.socket.as_deref(),
				config.port,
			)
			.await?;
			Ok(Prepared {
				path: source,
				extra_tags: metadata_tags(&resolved, strategy),
				ignore: ignore_globs(),
				teardown: Teardown::BaseBackup(root),
			})
		}
		Strategy::Vss => bail!(
			"the Windows VSS backup backend is not implemented yet"
		),
	}
}

/// Restore a postgres cluster from a freshly-restored tree (`staging`): stop the
/// cluster, swap the data directory into place (keeping the old one as
/// `<data>.old`), start it via plain crash recovery, and verify.
///
/// Refuses to overwrite an existing data directory unless `opts.clobber` is set
/// (the command sets it from the flag or an interactive confirmation).
pub async fn restore(
	config: &PostgresqlConfig,
	staging: &Path,
	opts: &super::method::RestoreOpts,
) -> Result<()> {
	let target = resolve::resolve_target(config)?;
	let restored = resolve::locate_pgdata(staging)?;
	info!(
		cluster = %target.cluster,
		version = %target.version,
		data_dir = %target.data_dir.display(),
		"restoring postgres cluster",
	);

	super::method::ensure_not_clobbering(&target.data_dir, opts.clobber)?;

	stop_cluster(&target).await;

	super::method::replace_dir(&restored, &target.data_dir).await?;
	fix_ownership(&target.data_dir).await?;

	if let Err(err) = start_cluster(&target).await {
		warn!(
			"cluster did not start cleanly ({err}); resetting WAL as a last resort \
			 (this may indicate a non-clean backup)"
		);
		pg_resetwal(&target.data_dir).await?;
		start_cluster(&target).await?;
	}

	verify(config).await;
	info!("restore complete; run migrations / config sync as needed");
	Ok(())
}

async fn stop_cluster(target: &resolve::ResolvedCluster) {
	let unit = format!("postgresql@{}-{}", target.version, target.cluster);
	if let Err(err) = run_status("systemctl", &["stop", &unit]).await {
		warn!("stopping {unit} failed (continuing): {err}");
	}
}

async fn start_cluster(target: &resolve::ResolvedCluster) -> Result<()> {
	let unit = format!("postgresql@{}-{}", target.version, target.cluster);
	run_status("systemctl", &["start", &unit]).await
}

async fn fix_ownership(data_dir: &Path) -> Result<()> {
	run_status("chown", &["-R", "postgres:postgres", path(data_dir)]).await?;
	run_status("chmod", &["0750", path(data_dir)]).await
}

async fn pg_resetwal(data_dir: &Path) -> Result<()> {
	let bin = crate::find_postgres::find_postgres_bin("pg_resetwal")
		.map(|p| p.to_string_lossy().into_owned())
		.unwrap_or_else(|_| "pg_resetwal".to_owned());
	run_status("sudo", &["-u", "postgres", &bin, "-f", path(data_dir)]).await
}

async fn verify(config: &PostgresqlConfig) {
	let psql = crate::find_postgres::find_postgres_bin("psql")
		.map(|p| p.to_string_lossy().into_owned())
		.unwrap_or_else(|_| "psql".to_owned());
	let mut cmd = tokio::process::Command::new("sudo");
	cmd.args(["-u", "postgres", &psql, "-X", "-q", "-tAc", "SELECT 1"]);
	if let Some(port) = config.port {
		cmd.arg("-p").arg(port.to_string());
	}
	cmd.stdin(std::process::Stdio::null());
	match cmd.status().await {
		Ok(s) if s.success() => info!("restored cluster accepts connections"),
		Ok(s) => warn!(%s, "post-restore verification query failed"),
		Err(err) => warn!("could not run verification query: {err}"),
	}
}

fn path(p: &Path) -> &str {
	p.to_str().unwrap_or_default()
}

async fn run_status(program: &str, args: &[&str]) -> Result<()> {
	let status = tokio::process::Command::new(program)
		.args(args)
		.stdin(std::process::Stdio::null())
		.status()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("spawning {program}"))?;
	if !status.success() {
		bail!("{program} {} failed ({status})", args.join(" "));
	}
	Ok(())
}

/// Best-effort `CHECKPOINT` as the postgres superuser over the local socket.
async fn checkpoint(config: &PostgresqlConfig) {
	let psql = crate::find_postgres::find_postgres_bin("psql")
		.map(|p| p.to_string_lossy().into_owned())
		.unwrap_or_else(|_| "psql".to_owned());

	// Run as the `postgres` OS user (peer auth) since the backup runs elevated.
	let mut cmd = tokio::process::Command::new("sudo");
	cmd.args(["-u", "postgres", &psql, "-X", "-q"]);
	if let Some(socket) = &config.socket {
		cmd.arg("-h").arg(socket);
	}
	if let Some(port) = config.port {
		cmd.arg("-p").arg(port.to_string());
	}
	cmd.args(["-c", "CHECKPOINT;"]);
	cmd.stdin(std::process::Stdio::null());

	match cmd.status().await {
		Ok(status) if status.success() => info!("issued CHECKPOINT before snapshot"),
		Ok(status) => warn!(
			%status,
			"CHECKPOINT failed; snapshot is still crash-consistent, recovery may just replay more WAL"
		),
		Err(err) => warn!("could not run CHECKPOINT (continuing): {err}"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn ignore_globs_never_include_required_dirs() {
		let globs = ignore_globs();
		assert!(globs.contains(&"postmaster.pid".to_owned()));
		for required in ["pg_wal", "pg_xact", "pg_control", "global"] {
			assert!(
				!globs.iter().any(|g| g.contains(required)),
				"{required} must never be ignored"
			);
		}
	}

	#[test]
	fn metadata_tags_carry_version_cluster_strategy() {
		let resolved = resolve::ResolvedCluster {
			data_dir: "/var/lib/postgresql/16/main".into(),
			version: "16".into(),
			cluster: "main".into(),
		};
		let tags = metadata_tags(&resolved, Strategy::Btrfs);
		assert_eq!(tags.get("pg-version").map(String::as_str), Some("16"));
		assert_eq!(tags.get("pg-cluster").map(String::as_str), Some("main"));
		assert_eq!(tags.get("pg-strategy").map(String::as_str), Some("btrfs"));
	}
}
