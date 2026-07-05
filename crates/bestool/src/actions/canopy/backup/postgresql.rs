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
mod service;
pub mod strategy;
mod sys;
pub mod vss;

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
	#[cfg(unix)]
	{
		// Under the daemon's root-owned StateDirectory (/var/lib/bestool), not the
		// kopia user's home: the daemon (root, without DAC write-override) creates
		// the snapshot mount / base-backup staging here, then hands it to the kopia
		// user. /var/lib/bestool is world-traversable so kopia can still read in.
		PathBuf::from("/var/lib/bestool/backup-source").join(backup_type)
	}
	#[cfg(not(unix))]
	{
		let base = std::env::var_os("ProgramData")
			.map(PathBuf::from)
			.unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
		base.join("bestool").join("backup-source").join(backup_type)
	}
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
	checkpoint(config, &resolved.data_dir).await;

	match strategy {
		Strategy::BaseBackup => basebackup_prepared(&resolved, backup_type, config).await,
		// For a snapshot backend (btrfs/thin-LVM/VSS): if the snapshot can't be
		// taken — VSS unavailable, missing privileges, a layout we can't capture
		// atomically — fall back to pg_basebackup rather than fail. That's a safe
		// degradation (a correct, if heavier, base backup) — never the live dir.
		snapshot => match snapshot_prepared(snapshot, &resolved, backup_type).await {
			Ok(prepared) => Ok(prepared),
			Err(err) => {
				warn!(
					strategy = ?snapshot,
					"snapshot backend unavailable ({err}); falling back to pg_basebackup"
				);
				basebackup_prepared(&resolved, backup_type, config).await
			}
		},
	}
}

/// Prepare via a snapshot backend (btrfs / thin-LVM / VSS).
async fn snapshot_prepared(
	strategy: Strategy,
	resolved: &resolve::ResolvedCluster,
	backup_type: &str,
) -> Result<Prepared> {
	let (path, teardown) = match strategy {
		Strategy::Btrfs => {
			let (path, mounts) = btrfs::prepare(resolved, backup_type).await?;
			(path, Teardown::Btrfs(mounts))
		}
		Strategy::ThinLvm => {
			let (path, snapshot) = lvm::prepare(resolved, backup_type).await?;
			(path, Teardown::Lvm(snapshot))
		}
		Strategy::Vss => {
			let (path, shadow) = vss::prepare(resolved, backup_type).await?;
			(path, Teardown::Vss(shadow))
		}
		Strategy::BaseBackup => unreachable!("basebackup is handled by the caller"),
	};
	Ok(Prepared {
		path,
		extra_tags: metadata_tags(resolved, strategy),
		ignore: ignore_globs(),
		teardown,
	})
}

/// Prepare via `pg_basebackup` (the always-correct fallback).
async fn basebackup_prepared(
	resolved: &resolve::ResolvedCluster,
	backup_type: &str,
	config: &PostgresqlConfig,
) -> Result<Prepared> {
	let (path, root) = basebackup::prepare(resolved, backup_type, config).await?;
	Ok(Prepared {
		path,
		// Tagged as basebackup even on fallback — it reflects what actually ran.
		extra_tags: metadata_tags(resolved, Strategy::BaseBackup),
		ignore: ignore_globs(),
		teardown: Teardown::BaseBackup(root),
	})
}

/// Restore a postgres cluster from a freshly-restored tree (`staging`): stop the
/// cluster, swap the restored tree into place (keeping the old one as
/// `<dest>.old`), start it via plain crash recovery, and verify.
///
/// A Windows backup carries the whole server install, so the swap replaces the
/// `PostgreSQL\<version>` directory and the exact matching binaries come with it.
/// A data-only backup (Linux, legacy Windows) replaces just the data directory,
/// so the matching server major version must already be installed — checked up
/// front. The version is taken from the restored `PG_VERSION`, not the target
/// path, so the systemd unit / service name match the data.
///
/// Refuses to overwrite an existing directory unless `opts.clobber` is set (the
/// command sets it from the flag or an interactive confirmation).
pub async fn restore(
	config: &PostgresqlConfig,
	staging: &Path,
	opts: &super::method::RestoreOpts,
) -> Result<()> {
	// The plan targets the snapshot's *own* major version (from PG_VERSION), so the
	// destination, the service stopped/started, and the binaries all match the data
	// being restored — even when a different major is the currently-installed cluster.
	let plan = resolve::plan_restore(staging, config)?;
	let target = &plan.target;
	info!(
		cluster = %target.cluster,
		version = %target.version,
		dest = %plan.dest.display(),
		whole_install = plan.whole_install,
		"restoring postgres cluster",
	);

	super::method::ensure_not_clobbering(&plan.dest, opts.clobber)?;

	// A data-only backup carries no binaries; a physical restore only runs under
	// its own major version. Fail-and-prompt so the operator can install it and
	// retry (the recheck runs each attempt). A whole-install backup brings its own.
	if !plan.whole_install {
		let major = plan.data_major.clone();
		crate::interactive::retry("checking the installed postgres version", async || {
			resolve::ensure_server_version_available(&major)
		})
		.await?;
	}

	// Stop the cluster before swapping: on Windows an open handle to the running
	// server's files makes the move fail outright; on Unix it would corrupt a live
	// cluster. Both this and the swap depend on nothing else holding the files, so
	// let the operator clear a stubborn holder by hand and retry — each attempt
	// re-checks, so the stop can't be skipped.
	crate::interactive::retry("stopping the postgres cluster", async || {
		service::stop(target, config).await
	})
	.await?;

	// Quiesce the other installed postgres versions' services (stop + set to manual
	// start) so a differently-versioned server can't hold the port or auto-restart
	// over the cluster we're restoring. Best-effort.
	service::quiesce_other_versions(&target.version).await;

	crate::interactive::retry("moving the restored data into place", async || {
		super::method::replace_dir(&plan.source, &plan.dest).await
	})
	.await?;

	crate::interactive::retry("fixing data directory ownership", async || {
		fix_ownership(&target.data_dir).await
	})
	.await?;

	// A crash-consistent restore normally starts via ordinary crash recovery. If
	// it won't, resetting the WAL forces a start but is a destructive last resort,
	// so it's an explicit operator choice rather than automatic.
	crate::interactive::retry_or_recover(
		"starting the postgres cluster",
		"reset the write-ahead log",
		"force-reset the WAL so the cluster can start without replaying it — \
		 destructive: can discard recent transactions or corrupt an \
		 otherwise-healthy cluster; only sound for a backup that won't start any \
		 other way",
		async || service::start(target, config).await,
		async || pg_resetwal(&target.data_dir, &target.version).await,
	)
	.await?;

	// The BES Linux layout indirects the active cluster through
	// `/var/lib/postgresql/current` and `/etc/postgresql/current`; point them at the
	// restored version so `current`-based consumers follow it (a no-op for a
	// same-major restore, where they already resolve here by path).
	repoint_current_symlinks(target).await;

	verify(config, &target.data_dir, &target.version).await;
	info!("restore complete; run migrations / config sync as needed");
	Ok(())
}

/// Repoint the BES `current` symlinks at the restored version, so consumers that
/// resolve the cluster through `/var/lib/postgresql/current` (the data dir) and
/// `/etc/postgresql/current` (the version config) follow a restore that changes
/// the active major. Only ever *repoints an existing* symlink (never imposes the
/// convention on a host that doesn't use it), and only points `/etc` at a config
/// directory that exists. Best-effort; Unix-only.
#[cfg(unix)]
async fn repoint_current_symlinks(target: &resolve::ResolvedCluster) {
	repoint_symlink_if_present(&resolve::postgres_base().join("current"), &target.data_dir).await;

	let etc_version = PathBuf::from("/etc/postgresql").join(&target.version);
	if etc_version.is_dir() {
		repoint_symlink_if_present(Path::new("/etc/postgresql/current"), &etc_version).await;
	}
}

#[cfg(not(unix))]
async fn repoint_current_symlinks(_target: &resolve::ResolvedCluster) {}

/// Atomically repoint `link` at `dest`, but only when `link` already exists and is
/// a symlink. Best-effort: a failure is warned, not fatal.
#[cfg(unix)]
async fn repoint_symlink_if_present(link: &Path, dest: &Path) {
	match tokio::fs::symlink_metadata(link).await {
		Ok(meta) if meta.file_type().is_symlink() => {}
		_ => return, // absent, or not a symlink: the convention isn't in play here
	}
	// Stage a new symlink beside it and rename over the old one, so the swap is
	// atomic (no window where `link` is missing).
	let staged = link.with_extension("bestool-current");
	let _ = tokio::fs::remove_file(&staged).await;
	if let Err(err) = tokio::fs::symlink(dest, &staged).await {
		warn!("could not stage symlink {} -> {}: {err}", staged.display(), dest.display());
		return;
	}
	if let Err(err) = tokio::fs::rename(&staged, link).await {
		warn!("could not repoint {} -> {}: {err}", link.display(), dest.display());
		let _ = tokio::fs::remove_file(&staged).await;
	} else {
		info!("repointed {} -> {}", link.display(), dest.display());
	}
}

/// Restore the postgres-owned mode and ownership of the freshly-swapped data
/// directory. Unix-only: on Windows the directory inherits its parent's ACLs
/// (the EDB install root), which is what the service account already expects.
#[cfg(unix)]
async fn fix_ownership(data_dir: &Path) -> Result<()> {
	run_status("chown", &["-R", "postgres:postgres", path(data_dir)]).await?;
	run_status("chmod", &["0750", path(data_dir)]).await
}

#[cfg(not(unix))]
async fn fix_ownership(_data_dir: &Path) -> Result<()> {
	Ok(())
}

async fn pg_resetwal(data_dir: &Path, major: &str) -> Result<()> {
	let mut cmd = pg_command(&postgres_bin_versioned("pg_resetwal", major, data_dir));
	cmd.arg("-f").arg(data_dir);
	run_checked(cmd, "pg_resetwal").await
}

async fn verify(config: &PostgresqlConfig, data_dir: &Path, major: &str) {
	let mut cmd = pg_command(&postgres_bin_versioned("psql", major, data_dir));
	// -w as in `checkpoint`: never block on a terminal password prompt.
	cmd.args(["-X", "-q", "-w", "-tAc", "SELECT 1"]);
	apply_connection(&mut cmd, config);
	cmd.stdin(std::process::Stdio::null());
	match cmd.status().await {
		Ok(s) if s.success() => info!("restored cluster accepts connections"),
		Ok(s) => warn!(%s, "post-restore verification query failed"),
		Err(err) => warn!("could not run verification query: {err}"),
	}
}

#[cfg(unix)]
fn path(p: &Path) -> &str {
	p.to_str().unwrap_or_default()
}

#[cfg(unix)]
pub(super) async fn run_status(program: &str, args: &[&str]) -> Result<()> {
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

/// Locate a postgres binary.
///
/// On Windows the bins aren't on `PATH`; they sit beside the data dir in the
/// EDB layout (`<data_dir>\..\bin`, wherever the install is rooted), so look
/// there first. Otherwise fall back to the standard-install search.
pub(super) fn postgres_bin(name: &str, data_dir: &Path) -> String {
	#[cfg(windows)]
	if let Some(candidate) = bin_beside_data_dir(name, data_dir).filter(|p| p.is_file()) {
		return candidate.to_string_lossy().into_owned();
	}
	#[cfg(not(windows))]
	let _ = data_dir;

	crate::find_postgres::find_postgres_bin(name)
		.map(|p| p.to_string_lossy().into_owned())
		.unwrap_or_else(|_| name.to_owned())
}

/// Locate a postgres binary for a specific major version — for restore, where the
/// tool must match the restored data, not just any install. On Unix that's the
/// versioned install dir (`/usr/lib/postgresql/<major>/bin/<name>`), avoiding
/// [`postgres_bin`]'s highest-version fallback when several majors are installed.
/// On Windows the versioned bin already sits beside the data dir, so this defers
/// to [`postgres_bin`].
fn postgres_bin_versioned(name: &str, major: &str, data_dir: &Path) -> String {
	#[cfg(unix)]
	{
		let candidate = PathBuf::from("/usr/lib/postgresql")
			.join(major)
			.join("bin")
			.join(name);
		if candidate.is_file() {
			return candidate.to_string_lossy().into_owned();
		}
	}
	#[cfg(not(unix))]
	let _ = major;

	postgres_bin(name, data_dir)
}

/// The EDB-layout binary path beside the data dir (`<data_dir>\..\bin\<name>`).
#[cfg(any(windows, test))]
fn bin_beside_data_dir(name: &str, data_dir: &Path) -> Option<PathBuf> {
	let exe = if cfg!(windows) {
		format!("{name}.exe")
	} else {
		name.to_owned()
	};
	data_dir.parent().map(|p| p.join("bin").join(exe))
}

/// A command that runs a postgres tool as the right user: `sudo -u postgres` on
/// Unix (peer auth + superuser/replication privilege), directly on Windows.
pub(super) fn pg_command(bin: &str) -> tokio::process::Command {
	#[cfg(unix)]
	{
		let mut cmd = tokio::process::Command::new("sudo");
		cmd.args(["-u", "postgres", bin]);
		cmd
	}
	#[cfg(not(unix))]
	{
		tokio::process::Command::new(bin)
	}
}

/// Apply connection params to a libpq client command (`psql`, `pg_basebackup`).
/// A configured `connection_url` (libpq URI / conninfo) carries the role, host
/// and credentials and takes over; otherwise fall back to the `socket` / `port`
/// flags and libpq's defaults for the rest.
pub(super) fn apply_connection(cmd: &mut tokio::process::Command, config: &PostgresqlConfig) {
	if let Some(url) = &config.connection_url {
		cmd.arg("-d").arg(url);
		return;
	}
	if let Some(socket) = &config.socket {
		cmd.arg("-h").arg(socket);
	}
	if let Some(port) = config.port {
		cmd.arg("-p").arg(port.to_string());
	}
}

/// Run a prepared command, erroring on non-zero exit.
async fn run_checked(mut cmd: tokio::process::Command, what: &str) -> Result<()> {
	let status = cmd
		.stdin(std::process::Stdio::null())
		.status()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("spawning {what}"))?;
	if !status.success() {
		bail!("{what} failed ({status})");
	}
	Ok(())
}

/// Best-effort `CHECKPOINT` as the postgres superuser over the local socket.
async fn checkpoint(config: &PostgresqlConfig, data_dir: &Path) {
	let mut cmd = pg_command(&postgres_bin("psql", data_dir));
	// -w: never prompt for a password. libpq reads a password prompt straight from
	// the terminal, not stdin, so null stdin alone doesn't stop it — without -w a
	// connection that needs a password (e.g. as the OS user on Windows) blocks the
	// service forever. With -w it fails fast instead, and CHECKPOINT is best-effort.
	cmd.args(["-X", "-q", "-w"]);
	apply_connection(&mut cmd, config);
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

	#[cfg(unix)]
	#[tokio::test]
	async fn repoint_symlink_updates_an_existing_symlink() {
		let tmp = tempfile::tempdir().unwrap();
		let old = tmp.path().join("18");
		let new = tmp.path().join("17");
		std::fs::create_dir_all(&old).unwrap();
		std::fs::create_dir_all(&new).unwrap();
		let link = tmp.path().join("current");
		std::os::unix::fs::symlink(&old, &link).unwrap();

		repoint_symlink_if_present(&link, &new).await;
		assert_eq!(std::fs::read_link(&link).unwrap(), new);
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn repoint_symlink_leaves_a_non_symlink_alone() {
		let tmp = tempfile::tempdir().unwrap();
		// A real directory at the link path must not be touched.
		let real = tmp.path().join("current");
		std::fs::create_dir_all(&real).unwrap();
		let dest = tmp.path().join("target");
		std::fs::create_dir_all(&dest).unwrap();

		repoint_symlink_if_present(&real, &dest).await;
		assert!(real.is_dir());
		assert!(!std::fs::symlink_metadata(&real).unwrap().file_type().is_symlink());
	}

	#[test]
	fn bin_beside_data_dir_is_sibling_of_data() {
		let candidate = bin_beside_data_dir("pg_basebackup", Path::new("/opt/pg/16/data")).unwrap();
		let name = if cfg!(windows) {
			"pg_basebackup.exe"
		} else {
			"pg_basebackup"
		};
		assert_eq!(candidate, Path::new("/opt/pg/16/bin").join(name));
	}

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

	fn pg_config(connection_url: Option<&str>, socket: Option<&str>, port: Option<u16>) -> PostgresqlConfig {
		PostgresqlConfig {
			cluster: "main".into(),
			data_dir: None,
			version: None,
			connection_url: connection_url.map(str::to_owned),
			port,
			socket: socket.map(PathBuf::from),
			strategy: None,
			service_name: None,
		}
	}

	#[test]
	fn apply_connection_prefers_the_url() {
		let mut cmd = tokio::process::Command::new("psql");
		apply_connection(&mut cmd, &pg_config(Some("postgresql://u:p@h/db"), Some("/run/pg"), Some(5433)));
		let args: Vec<_> = cmd
			.as_std()
			.get_args()
			.map(|a| a.to_string_lossy().into_owned())
			.collect();
		assert_eq!(args, vec!["-d", "postgresql://u:p@h/db"]);
	}

	#[test]
	fn apply_connection_falls_back_to_socket_and_port() {
		let mut cmd = tokio::process::Command::new("psql");
		apply_connection(&mut cmd, &pg_config(None, Some("/run/pg"), Some(5433)));
		let args: Vec<_> = cmd
			.as_std()
			.get_args()
			.map(|a| a.to_string_lossy().into_owned())
			.collect();
		assert_eq!(args, vec!["-h", "/run/pg", "-p", "5433"]);
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
