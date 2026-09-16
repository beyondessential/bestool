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
pub mod space;
pub mod strategy;
mod sys;
pub mod usn;
#[cfg(windows)]
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

	// The base-backup fallback stages a full copy, and VSS needs copy-on-write
	// room, so estimate the cluster size up front for those two; the snapshot
	// backends capture in place and need no space reservation.
	let need = match strategy {
		Strategy::BaseBackup | Strategy::Vss => {
			space::estimate_needed(config, &resolved.data_dir).await
		}
		_ => None,
	};

	match strategy {
		Strategy::BaseBackup => basebackup_prepared(&resolved, backup_type, config, need).await,
		// For a snapshot backend (btrfs/thin-LVM/VSS): if the snapshot can't be
		// taken — VSS unavailable, missing privileges, a layout we can't capture
		// atomically — fall back to pg_basebackup rather than fail. That's a safe
		// degradation (a correct, if heavier, base backup) — never the live dir.
		snapshot => match snapshot_prepared(snapshot, &resolved, backup_type, need).await {
			Ok(prepared) => Ok(prepared),
			Err(err) => {
				// Render the whole cause chain: `Display` shows only the outermost
				// context ("creating VSS shadow copy"), hiding the operative reason
				// (the diskshadow/lvs error) that says *why* the snapshot failed.
				warn!(
					strategy = ?snapshot,
					"snapshot backend unavailable ({}); falling back to pg_basebackup",
					super::trim_error(&err)
				);
				basebackup_prepared(&resolved, backup_type, config, need).await
			}
		},
	}
}

/// Prepare via a snapshot backend (btrfs / thin-LVM / VSS).
async fn snapshot_prepared(
	strategy: Strategy,
	resolved: &resolve::ResolvedCluster,
	backup_type: &str,
	need: Option<u64>,
) -> Result<Prepared> {
	let (path, taken_at, teardown, volume) = match strategy {
		Strategy::Btrfs => {
			let (path, taken_at, mounts) = btrfs::prepare(resolved, backup_type).await?;
			(path, taken_at, Teardown::Btrfs(mounts), None)
		}
		Strategy::ThinLvm => {
			let (path, taken_at, snapshot) = lvm::prepare(resolved, backup_type).await?;
			(path, taken_at, Teardown::Lvm(snapshot), None)
		}
		#[cfg(windows)]
		Strategy::Vss => {
			let (path, taken_at, shadow) = vss::prepare(resolved, backup_type, need).await?;
			let volume = vss::volume_capture(&shadow, taken_at);
			(path, taken_at, Teardown::Vss(shadow), volume)
		}
		#[cfg(not(windows))]
		Strategy::Vss => {
			let _ = need;
			unreachable!("VSS is only detected on Windows")
		}
		Strategy::BaseBackup => unreachable!("basebackup is handled by the caller"),
	};
	Ok(Prepared {
		path,
		taken_at: Some(taken_at),
		extra_tags: metadata_tags(resolved, strategy),
		ignore: ignore_globs(),
		teardown,
		volume,
	})
}

/// Prepare via `pg_basebackup` (the always-correct fallback).
async fn basebackup_prepared(
	resolved: &resolve::ResolvedCluster,
	backup_type: &str,
	config: &PostgresqlConfig,
	need: Option<u64>,
) -> Result<Prepared> {
	let (path, root) = basebackup::prepare(resolved, backup_type, config, need).await?;
	Ok(Prepared {
		path,
		// A streamed base backup represents an interval, not a point in time, so it
		// reports no freeze instant.
		taken_at: None,
		// Tagged as basebackup even on fallback — it reflects what actually ran.
		extra_tags: metadata_tags(resolved, Strategy::BaseBackup),
		ignore: ignore_globs(),
		teardown: Teardown::BaseBackup(root),
		volume: None,
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

	stop_the_cluster(config, &plan).await?;

	crate::interactive::retry("moving the restored data into place", async || {
		super::method::replace_dir(&plan.source, &plan.dest).await
	})
	.await?;

	start_the_cluster(config, &plan).await?;
	verify(config, &target.data_dir, &target.version).await;
	info!("restore complete; run migrations / config sync as needed");
	Ok(())
}

/// Bring the cluster down so its data directory can be written.
///
/// Shared by the staged and in-place paths: both depend on nothing holding the
/// files, and on no other installed version starting over the one being
/// restored. Keeping it in one place is what stops the two drifting.
async fn stop_the_cluster(config: &PostgresqlConfig, plan: &resolve::RestorePlan) -> Result<()> {
	let target = &plan.target;

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

	// Stop the cluster before writing: on Windows an open handle to the running
	// server's files makes the move fail outright; on Unix it would corrupt a live
	// cluster. This depends on nothing else holding the files, so let the operator
	// clear a stubborn holder by hand and retry — each attempt re-checks, so the
	// stop can't be skipped.
	crate::interactive::retry("stopping the postgres cluster", async || {
		service::stop(target, config).await
	})
	.await?;

	// Quiesce the other installed postgres versions' services (stop + set to manual
	// start) so a differently-versioned server can't hold the port or auto-restart
	// over the cluster we're restoring. Best-effort.
	service::quiesce_other_versions(&target.version).await;
	Ok(())
}

/// Bring the restored cluster back up, and point the layout's symlinks at it.
async fn start_the_cluster(config: &PostgresqlConfig, plan: &resolve::RestorePlan) -> Result<()> {
	let target = &plan.target;

	// A tree some in-place attempt left part-way through is neither state, and
	// starting it corrupts it. Checked on both paths rather than only the one
	// that writes markers: a staged restore over a marked tree would otherwise
	// start it, and the marker's whole purpose is to say do not.
	#[cfg(feature = "canopy-restore")]
	crate::actions::canopy::restore::inplace::Interlock::ensure_clear(&target.data_dir).await?;

	// The account the server runs as, so its files can be made writable by it
	// (Windows). Fix the whole restored tree (`dest`): the data dir for a data-only
	// restore, or the install root — binaries included — for a whole-install one.
	let service_account = service::service_account(target, config).await;
	crate::interactive::retry("fixing restored data permissions", async || {
		fix_ownership(&plan.dest, service_account.as_deref()).await
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

/// Fix up ownership and permissions of the freshly-restored tree (`dest`) so the
/// postgres server account can read and write it.
///
/// On Unix: `chown` to `postgres` and `chmod 0750` — the peer-auth service user.
///
/// On Windows: kopia can restore files with an ACL that grants neither the
/// service account nor Administrators, so the server can't even create
/// `postmaster.pid` and won't start. Take ownership (the elevated caller always
/// can), reset each entry to the ACL inherited from the install root, and grant
/// the server's own account (`account`, e.g. `NT AUTHORITY\NetworkService`) full
/// control.
#[cfg(unix)]
async fn fix_ownership(dest: &Path, _account: Option<&str>) -> Result<()> {
	run_status("chown", &["-R", "postgres:postgres", path(dest)]).await?;
	run_status("chmod", &["0750", path(dest)]).await
}

#[cfg(windows)]
async fn fix_ownership(dest: &Path, account: Option<&str>) -> Result<()> {
	let dir = dest.to_string_lossy();
	// Take ownership so the ACL can be rewritten even when kopia locked it down.
	run_status("takeown", &["/F", dir.as_ref(), "/R", "/D", "Y"]).await?;
	// Reset every entry to the ACL inherited from the install root.
	run_status("icacls", &[dir.as_ref(), "/reset", "/T", "/C", "/Q"]).await?;
	// Grant the server's own account full control (object + container inherit).
	if let Some(account) = account {
		let grant = format!("{account}:(OI)(CI)F");
		run_status("icacls", &[dir.as_ref(), "/grant", grant.as_str(), "/T", "/C", "/Q"]).await?;
	}
	Ok(())
}

#[cfg(not(any(unix, windows)))]
async fn fix_ownership(_dest: &Path, _account: Option<&str>) -> Result<()> {
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

#[cfg(any(unix, windows))]
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
			staging_dir: None,
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

/// Restore a postgres cluster from a held capture without staging a copy of it.
///
/// The staged path needs free space for a whole second cluster, and leaves the
/// tree it displaced behind as `<dest>.old`, so a completed restore needs about
/// half again the cluster on the volume. Where the capture is a snapshot of that
/// same volume, this instead writes only what diverged from it, which is hours
/// of writes rather than the size of the database.
///
/// What it gives up is the atomic swap. There is no `<dest>.old` to fall back
/// to, and while it runs the tree is neither the state it was in nor the state
/// that was captured. A data directory in that condition is corrupt if started,
/// so `PG_VERSION` is taken out of the way first and written back from the
/// capture last: postgres will not start a cluster without it. The hold is
/// untouched throughout — reads consume no copy-on-write space — so running the
/// same command again resumes.
///
/// spec: HOLD#restoring-from-a-held-capture
#[cfg(feature = "canopy-restore")]
pub async fn restore_in_place(
	config: &PostgresqlConfig,
	record: &super::hold::HoldRecord,
	capture: &Path,
	opts: &super::method::RestoreOpts,
) -> Result<()> {
	use crate::actions::canopy::restore::inplace::{Interlock, Job};

	// The capture stands in for the staging tree: it has the same shape, which is
	// what lets one plan serve both paths.
	let plan = resolve::plan_restore(capture, config)?;
	let target = &plan.target;
	info!(
		cluster = %target.cluster,
		version = %target.version,
		dest = %plan.dest.display(),
		whole_install = plan.whole_install,
		hold = %record.id,
		"restoring postgres cluster in place from a held capture",
	);

	// A tree left part-way through *this* restore is not "existing data" an
	// operator is about to lose; it is the same restore being finished, and
	// asking to clobber it again would make the documented recovery need a flag
	// the first attempt did not. A marker naming any other hold is not that, and
	// neither is one someone dropped into the data directory, so it stands in for
	// consent only when it names the hold being restored.
	//
	// A confirmation that was *refused* is a different matter: the marker stands
	// in for one nobody was asked for, never for one the operator said no to.
	let resuming = Interlock::resumes(&target.data_dir, record).await;
	if !resuming || opts.declined {
		super::method::ensure_not_clobbering_in_place(&plan.dest, opts.clobber)?;
	}

	stop_the_cluster(config, &plan).await?;

	// Everything that can refuse happens while the cluster is merely stopped:
	// `PG_VERSION` is still in place and no marker is written, so a refusal here
	// leaves a cluster an operator can simply start again. The comparison has to
	// come after the stop, though — a delta worked out against a running cluster
	// would miss whatever it wrote between the walk and the stop.
	//
	// One pass over the whole tree being replaced, which on a whole-install
	// capture is the install directory rather than the data directory alone: its
	// binaries are part of the captured state and must roll back with the data
	// they match. Walking the data directory and then the shell around it
	// separately would resolve the basis twice — a second `find-new`, or a second
	// pass over the change journal — and gate the two halves against free space
	// independently, which two passes can each fit without the pair fitting.
	let planned = super::super::restore::inplace::plan(Job {
		record,
		capture: &plan.source,
		live: &plan.dest,
		skip: interlock_skips(&plan)?,
	})
	.await
	.wrap_err_with(|| {
		format!(
			"nothing was written, and the cluster at {} is stopped but intact; \
			 start it again, or resolve this and re-run the restore",
			target.data_dir.display(),
		)
	})?;

	let version_file = target.data_dir.join("PG_VERSION");
	let parked = super::method::with_extension_suffix(&version_file, PARKED_SUFFIX);

	// Whether the cluster has to be held unstartable, which is *not* the same
	// question as whether there is anything to write. All three interlock files
	// are skipped by the walk, so an attempt interrupted after its last copy but
	// before `PG_VERSION` was written back leaves nothing for the next walk to
	// find — and deciding on the delta alone would then skip putting
	// `PG_VERSION` back and skip releasing the marker, on every re-run, leaving
	// no way out of a cluster that cannot start.
	let unfinished = resuming || tokio::fs::symlink_metadata(&parked).await.is_ok();
	let summary = if planned.has_work() || unfinished {
		// From here the tree is written over with no way back to what it was, so
		// the cluster is made unstartable before the first write rather than after.
		let interlock = Interlock::engage(&target.data_dir, record).await?;
		park_version_file(&version_file, &parked).await?;

		let summary = super::super::restore::inplace::lay_down(planned)
			.await
			.wrap_err_with(|| {
				format!(
					"the cluster at {} is part-way through a restore from hold {} and will \
					 not start; the hold is untouched, so run the same command again to \
					 finish it",
					target.data_dir.display(),
					record.id,
				)
			})?;

		// Last, so the cluster becomes startable only once everything under it is
		// the captured state. Reached even when the sync had nothing left to do,
		// because finishing an interrupted attempt is exactly that case.
		sync_version_file(&resolve::locate_pgdata(capture)?, &version_file, &parked).await?;
		interlock.release().await?;
		summary
	} else {
		// Nothing diverged and nothing was left unfinished, so the cluster was
		// never made unstartable and there is nothing to put back.
		super::super::restore::inplace::lay_down(planned).await?
	};

	info!(
		copied = summary.copied,
		removed = summary.removed,
		bytes = summary.bytes,
		from_filesystem = summary.from_filesystem,
		"the divergence from the held capture is laid down",
	);

	start_the_cluster(config, &plan).await?;

	verify(config, &target.data_dir, &target.version).await;
	info!("in-place restore complete; run migrations / config sync as needed");
	Ok(())
}

/// The suffix the pre-restore `PG_VERSION` is parked under for the duration.
#[cfg(feature = "canopy-restore")]
const PARKED_SUFFIX: &str = "bestool-restoring";

/// The entries the sync must not touch, relative to the root of the tree being
/// replaced.
///
/// `PG_VERSION` because it is written back from the capture last, so that the
/// cluster becomes startable only once everything beneath it is the captured
/// state. The parked copy of it and the in-flight marker because they are not
/// part of the captured state at all — a sync that saw them would remove them as
/// files written since the freeze, taking with them the two things keeping a
/// part-restored cluster from being started.
///
/// All three live in the data directory, which on a whole-install restore is
/// nested inside the tree being walked, so they are named from its root.
#[cfg(feature = "canopy-restore")]
fn interlock_skips(plan: &resolve::RestorePlan) -> Result<Vec<PathBuf>> {
	use crate::actions::canopy::restore::inplace::Interlock;

	let within = plan.target.data_dir.strip_prefix(&plan.dest).map_err(|_| {
		miette::miette!(
			"the data directory {} is not inside the tree being restored ({})",
			plan.target.data_dir.display(),
			plan.dest.display(),
		)
	})?;
	Ok(vec![
		within.join("PG_VERSION"),
		within.join(format!("PG_VERSION.{PARKED_SUFFIX}")),
		within.join(Interlock::marker_name()),
	])
}

/// Take `PG_VERSION` out of the way, so postgres cannot start the cluster while
/// it is being written over.
///
/// A parked copy already being there is the normal state of a resumed restore,
/// and a missing original is the normal state of a resumed one too, so neither
/// is an error. Both being absent is not: that would leave the cluster
/// startable through the whole sync.
#[cfg(feature = "canopy-restore")]
async fn park_version_file(version_file: &Path, parked: &Path) -> Result<()> {
	if tokio::fs::metadata(parked).await.is_ok() {
		// A previous attempt parked it; keep that copy, which is the pre-restore
		// one, and discard whatever a partial sync may have left in its place.
		//
		// The removal is the whole safety argument of this mode — postgres will
		// not start a cluster without `PG_VERSION` — so a failure to remove it is
		// a failure to hold the cluster down, not a tidying detail. Only its
		// absence is acceptable.
		match tokio::fs::remove_file(version_file).await {
			Ok(()) => {}
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
			Err(err) => {
				return Err(err).into_diagnostic().wrap_err_with(|| {
					format!(
						"{} could not be moved out of the way, so the cluster would stay \
						 startable while it is being written over",
						version_file.display()
					)
				});
			}
		}
		return Ok(());
	}
	match tokio::fs::rename(version_file, parked).await {
		Ok(()) => Ok(()),
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
		Err(err) => Err(err)
			.into_diagnostic()
			.wrap_err_with(|| format!("moving {} out of the way", version_file.display())),
	}
}

/// Write `PG_VERSION` back from the capture, making the cluster startable, and
/// drop the parked copy.
#[cfg(feature = "canopy-restore")]
async fn sync_version_file(capture_data: &Path, version_file: &Path, parked: &Path) -> Result<()> {
	let from = capture_data.join("PG_VERSION");
	crate::actions::canopy::restore::sync::copy_entry(&from, version_file, false)
		.await
		.wrap_err_with(|| {
			format!(
				"writing {} back from the capture, without which the cluster cannot start",
				version_file.display()
			)
		})?;
	let _ = tokio::fs::remove_file(parked).await;
	Ok(())
}

#[cfg(all(test, feature = "canopy-restore"))]
mod in_place_tests {
	use super::*;

	use crate::actions::canopy::restore::inplace::Interlock;

	fn plan(dest: &str, data_dir: &str) -> resolve::RestorePlan {
		resolve::RestorePlan {
			source: PathBuf::from("/capture"),
			dest: PathBuf::from(dest),
			data_major: "16".into(),
			whole_install: dest != data_dir,
			target: resolve::ResolvedCluster {
				version: "16".into(),
				cluster: "main".into(),
				data_dir: PathBuf::from(data_dir),
			},
		}
	}

	/// All three have to be left alone, and the two that are not captured state
	/// are the ones that keep a part-restored cluster from being started — so a
	/// sync that removed them would undo the interlock silently.
	#[test]
	fn the_interlock_keeps_its_own_files_out_of_the_sync() {
		let skips = interlock_skips(&plan("/pg/16/main", "/pg/16/main")).unwrap();
		assert!(skips.contains(&PathBuf::from("PG_VERSION")));
		assert!(skips.contains(&PathBuf::from("PG_VERSION.bestool-restoring")));
		assert!(skips.contains(&PathBuf::from(Interlock::marker_name())));
	}

	/// A whole-install restore walks the install directory, so the data
	/// directory's own files are a level down. Naming them from the wrong root
	/// would let the sync remove the two that hold the cluster unstartable.
	#[test]
	fn the_skips_are_named_from_the_root_of_the_tree_being_walked() {
		let skips = interlock_skips(&plan("/pg/16", "/pg/16/data")).unwrap();
		assert!(skips.contains(&PathBuf::from("data/PG_VERSION")));
		assert!(skips.contains(&PathBuf::from("data/PG_VERSION.bestool-restoring")));
		assert!(skips.contains(&PathBuf::from("data").join(Interlock::marker_name())));
	}

	/// The whole scheme rests on the data directory being inside the tree being
	/// replaced. If it is not, the interlock's files would not be skipped and the
	/// sync would quietly remove them, so the restore refuses instead.
	#[test]
	fn a_data_directory_outside_the_restored_tree_is_refused() {
		assert!(interlock_skips(&plan("/pg/16", "/elsewhere/data")).is_err());
	}

	/// The parked copy is the *pre-restore* version file. A resumed restore must
	/// keep it rather than park whatever a partial sync left behind, or a
	/// half-written one would be treated as the original.
	#[tokio::test]
	async fn a_resumed_restore_keeps_the_copy_already_parked() {
		let tmp = tempfile::tempdir().unwrap();
		let version_file = tmp.path().join("PG_VERSION");
		let parked = tmp.path().join("PG_VERSION.bestool-restoring");
		std::fs::write(&parked, "16").unwrap();
		std::fs::write(&version_file, "partial").unwrap();

		park_version_file(&version_file, &parked).await.unwrap();

		assert_eq!(std::fs::read_to_string(&parked).unwrap(), "16");
		assert!(
			!version_file.exists(),
			"the cluster must stay unstartable across the resume"
		);
	}

	#[tokio::test]
	async fn parking_takes_the_version_file_out_of_the_way() {
		let tmp = tempfile::tempdir().unwrap();
		let version_file = tmp.path().join("PG_VERSION");
		let parked = tmp.path().join("PG_VERSION.bestool-restoring");
		std::fs::write(&version_file, "16").unwrap();

		park_version_file(&version_file, &parked).await.unwrap();

		assert!(!version_file.exists());
		assert_eq!(std::fs::read_to_string(&parked).unwrap(), "16");
	}

	/// A cluster whose data directory is not there yet has nothing to park, which
	/// is not a failure: there is likewise nothing startable.
	#[tokio::test]
	async fn parking_an_absent_version_file_is_not_an_error() {
		let tmp = tempfile::tempdir().unwrap();
		park_version_file(
			&tmp.path().join("PG_VERSION"),
			&tmp.path().join("PG_VERSION.bestool-restoring"),
		)
		.await
		.unwrap();
	}

	/// Writing it back is what makes the cluster startable, so it is the last
	/// thing the restore does and it drops the parked copy with it.
	#[tokio::test]
	async fn writing_the_version_file_back_clears_the_parked_copy() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		let live = tmp.path().join("live");
		std::fs::create_dir_all(&capture).unwrap();
		std::fs::create_dir_all(&live).unwrap();
		std::fs::write(capture.join("PG_VERSION"), "16").unwrap();
		let version_file = live.join("PG_VERSION");
		let parked = live.join("PG_VERSION.bestool-restoring");
		std::fs::write(&parked, "16").unwrap();

		sync_version_file(&capture, &version_file, &parked).await.unwrap();

		assert_eq!(std::fs::read_to_string(&version_file).unwrap(), "16");
		assert!(!parked.exists());
	}
}
