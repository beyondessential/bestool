//! The `postgresql` backup method: physical, crash-consistent cluster snapshots.
//!
//! Generic postgres (no Tamanu coupling): driven by the `[postgresql]` config
//! table. Resolves the cluster's data directory, issues a best-effort
//! `CHECKPOINT` to bound WAL replay on restore, detects the storage backend, and
//! captures it. btrfs is implemented; thin-LVM / Windows VSS / `pg_basebackup`
//! land in follow-ups.

pub mod btrfs;
pub mod resolve;
pub mod strategy;

use std::collections::BTreeMap;

use miette::{Result, bail};
use tracing::{info, warn};

use self::strategy::Strategy;
use super::method::{PostgresqlConfig, Prepared, Teardown};

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
		other => bail!(
			"postgresql backup strategy {other:?} is not implemented yet (btrfs only for now)"
		),
	}
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
