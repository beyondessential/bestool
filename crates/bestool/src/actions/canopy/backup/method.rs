//! Built-in backup methods.
//!
//! A backup def selects exactly one method. The driver runs the def's `pre`
//! hooks, calls [`Method::prepare`] to get a kopia source path (plus any
//! method-supplied tags), snapshots it, then calls [`Method::cleanup`] and the
//! `post` hooks. `type` is just the Canopy-facing label; the method is what
//! decides *how* to produce a consistent source.

use std::{
	collections::BTreeMap,
	path::{Path, PathBuf},
	time::{Duration, Instant},
};

use jiff::Timestamp;
use miette::{Result, bail};
use serde::Deserialize;
use tracing::debug;

mod holders;

/// A source ready for kopia to snapshot, produced by [`Method::prepare`].
#[derive(Debug)]
pub struct Prepared {
	/// The path kopia should snapshot.
	pub path: PathBuf,
	/// The instant the source was frozen — the point in time the backup
	/// represents — when the method takes a point-in-time capture below kopia.
	/// `None` for a method with no distinct freeze instant (a streamed base
	/// backup, or a path snapshotted live).
	pub taken_at: Option<Timestamp>,
	/// Extra tags the method contributes (merged with the canopy-* tags and the
	/// def's own `[tags]`).
	pub extra_tags: BTreeMap<String, String>,
	/// kopia ignore globs the driver applies to the source before snapshotting
	/// (e.g. postgres transient files).
	pub ignore: Vec<String>,
	/// Method-specific teardown, run by [`Method::cleanup`].
	pub(super) teardown: Teardown,
}

/// What [`Method::cleanup`] has to undo for a prepared source.
#[derive(Debug)]
pub(super) enum Teardown {
	/// The simple method's kopia-readable view (bindfs mount or copy).
	Simple(super::simple::Cleanup),
	/// A btrfs snapshot + its mounts.
	Btrfs(super::postgresql::btrfs::Mounts),
	/// A thin-LVM snapshot + its mount.
	Lvm(super::postgresql::lvm::Snapshot),
	/// A Windows VSS shadow copy to delete.
	#[cfg(windows)]
	Vss(super::postgresql::vss::Shadow),
	/// A streamed base backup directory to remove.
	BaseBackup(PathBuf),
}

/// `[simple]` method: snapshot a path verbatim.
///
/// The path is either fixed (`path`) or resolved on every run by a command
/// (`path_command`), for sources whose location lives outside the def, e.g.
/// the Tamanu blob store root, a database-backed setting an administrator can
/// move (`bestool tamanu blob-root` prints it). Exactly one of the two.
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleConfig {
	/// The path kopia snapshots.
	#[serde(default)]
	pub path: Option<PathBuf>,
	/// A command (argv-style, no shell) whose output is the absolute path to
	/// snapshot and restore.
	#[serde(default)]
	pub path_command: Option<Vec<String>>,
}

impl SimpleConfig {
	/// Enforce exactly one of `path` / `path_command` at def load.
	pub fn validate(&self, backup_type: &str) -> Result<()> {
		match (&self.path, &self.path_command) {
			(Some(_), None) => Ok(()),
			(None, Some(command)) if !command.is_empty() => Ok(()),
			(None, Some(_)) => bail!("backup def '{backup_type}' has an empty [simple] path_command"),
			(None, None) => bail!(
				"backup def '{backup_type}' has a [simple] table with neither path nor path_command"
			),
			(Some(_), Some(_)) => bail!(
				"backup def '{backup_type}' has both [simple] path and path_command; exactly one is allowed"
			),
		}
	}

	/// The path to snapshot or restore: the fixed one, or the command's output.
	pub async fn resolve_path(&self) -> Result<PathBuf> {
		use miette::{Context as _, IntoDiagnostic as _};

		if let Some(path) = &self.path {
			return Ok(path.clone());
		}
		let command = self
			.path_command
			.as_ref()
			.expect("validated: path or path_command is set");
		let (program, args) = command
			.split_first()
			.expect("validated: path_command is not empty");
		let output = tokio::process::Command::new(program)
			.args(args)
			.output()
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("running path_command {program}"))?;
		if !output.status.success() {
			bail!(
				"path_command {program} exited with {}: {}",
				output.status,
				String::from_utf8_lossy(&output.stderr).trim()
			);
		}
		let stdout = String::from_utf8_lossy(&output.stdout);
		let path = stdout.trim();
		if path.is_empty() || path.lines().count() != 1 {
			bail!("path_command {program} must output exactly one line, got: {path:?}");
		}
		let path = PathBuf::from(path);
		if !path.is_absolute() {
			bail!("path_command {program} must output an absolute path, got {}", path.display());
		}
		Ok(path)
	}
}

/// `[postgresql]` method: physical, crash-consistent cluster snapshot.
///
/// Driven entirely by this table — generic postgres, no Tamanu coupling.
#[derive(Debug, Clone, Deserialize)]
pub struct PostgresqlConfig {
	/// The cluster name (e.g. `main`); resolves the data dir / connection.
	pub cluster: String,
	/// Override the resolved data directory.
	#[serde(default)]
	pub data_dir: Option<PathBuf>,
	/// Override the resolved major version.
	#[serde(default)]
	pub version: Option<String>,
	/// libpq connection URI / conninfo for the client commands (`CHECKPOINT`,
	/// `pg_basebackup`, restore verification). Takes precedence over `socket` /
	/// `port` and carries the role and credentials — needed where peer auth isn't
	/// available (e.g. Windows, which has no `sudo -u postgres`). May embed a
	/// password (then visible in the process's argv); a passwordless URI plus a
	/// pgpass file / `PGPASSWORD` avoids that.
	#[serde(default)]
	pub connection_url: Option<String>,
	/// Override the port used to connect for `CHECKPOINT` (ignored when
	/// `connection_url` is set).
	#[serde(default)]
	pub port: Option<u16>,
	/// Override the unix socket directory used to connect (ignored when
	/// `connection_url` is set).
	#[serde(default)]
	pub socket: Option<PathBuf>,
	/// Force a snapshot strategy instead of auto-detecting (for testing).
	#[serde(default)]
	pub strategy: Option<String>,
	/// Override where the `pg_basebackup` full copy is staged. When set it's used
	/// verbatim (and the run fails early if it lacks room); when unset the roomiest
	/// suitable disk is chosen automatically. Ignored by the snapshot backends,
	/// which capture in place.
	#[serde(default)]
	pub staging_dir: Option<PathBuf>,
	/// Override the service controlled around a restore. On Windows this is the
	/// service name (defaults to the EDB installer's `postgresql-x64-<version>`);
	/// ignored on Unix, where the systemd unit is derived from version + cluster.
	#[serde(default)]
	#[cfg_attr(
		not(windows),
		expect(dead_code, reason = "only read on Windows; Unix derives the systemd unit")
	)]
	pub service_name: Option<String>,
}

/// A built-in backup method, selected by the def's single method table.
#[derive(Debug, Clone)]
pub enum Method {
	Simple(SimpleConfig),
	Postgresql(PostgresqlConfig),
}

impl Method {
	/// The method's name, used in diagnostics.
	pub fn name(&self) -> &'static str {
		match self {
			Method::Simple(_) => "simple",
			Method::Postgresql(_) => "postgresql",
		}
	}

	/// Produce the source kopia will snapshot. `backup_type` is the def's label,
	/// used by methods that key stable paths on it (e.g. btrfs mount points).
	pub async fn prepare(&self, backup_type: &str) -> Result<Prepared> {
		match self {
			Method::Simple(config) => {
				let source = config.resolve_path().await?;
				let (path, cleanup) = super::simple::prepare(&source, backup_type).await?;
				Ok(Prepared {
					path,
					// A live view (bindfs mount or copy) has no point-in-time freeze.
					taken_at: None,
					extra_tags: BTreeMap::new(),
					ignore: Vec::new(),
					teardown: Teardown::Simple(cleanup),
				})
			}
			Method::Postgresql(config) => super::postgresql::prepare(config, backup_type).await,
		}
	}

	/// Release whatever `prepare` set up (snapshot, mount, staging dir).
	pub async fn cleanup(&self, prepared: Prepared) -> Result<()> {
		match prepared.teardown {
			Teardown::Simple(cleanup) => super::simple::teardown(cleanup).await,
			Teardown::Btrfs(mounts) => super::postgresql::btrfs::teardown(mounts).await,
			Teardown::Lvm(snapshot) => super::postgresql::lvm::teardown(snapshot).await,
			#[cfg(windows)]
			Teardown::Vss(shadow) => super::postgresql::vss::teardown(shadow).await,
			Teardown::BaseBackup(root) => super::postgresql::basebackup::teardown(root).await,
		}
	}

	/// A staging directory for the restore, colocated with the eventual target's
	/// filesystem so the final move is an atomic rename. Falls back to the temp
	/// dir if the target can't be resolved.
	pub async fn staging_dir(&self, target_override: Option<&Path>, pid: u32) -> Result<PathBuf> {
		let parent = match self {
			Method::Simple(config) => {
				let target = match target_override {
					Some(target) => target.to_path_buf(),
					None => config.resolve_path().await?,
				};
				target.parent().map(Path::to_path_buf)
			}
			Method::Postgresql(config) => super::postgresql::resolve::restore_staging_parent(config),
		};
		Ok(parent
			.unwrap_or_else(std::env::temp_dir)
			.join(format!(".bestool-restore.{pid}")))
	}

	/// Lay a restored snapshot (in `staging`) back down. Method-specific: the
	/// simple method places files at its path; postgresql does the full
	/// stop/swap/start.
	pub async fn restore(&self, staging: &Path, opts: &RestoreOpts) -> Result<()> {
		match self {
			Method::Simple(config) => {
				let target = match &opts.target {
					Some(target) => target.clone(),
					None => config.resolve_path().await?,
				};
				ensure_not_clobbering(&target, opts.clobber)?;
				replace_dir(staging, &target).await
			}
			Method::Postgresql(config) => super::postgresql::restore(config, staging, opts).await,
		}
	}
}

/// Options controlling a restore.
#[derive(Debug, Clone, Default)]
pub struct RestoreOpts {
	/// Override the destination (the simple method's path); ignored by postgresql,
	/// which always targets the configured cluster.
	pub target: Option<PathBuf>,
	/// Proceed even when the destination already holds data.
	pub clobber: bool,
}

/// Error unless `target` is safe to write (absent or empty) or `clobber` is set.
pub fn ensure_not_clobbering(target: &Path, clobber: bool) -> Result<()> {
	if clobber || !dir_has_entries(target) {
		return Ok(());
	}
	bail!(
		"{} already contains data; refusing to overwrite without confirmation \
		 (pass --clobber-existing-data-yes-i-am-sure, or confirm interactively)",
		target.display()
	);
}

/// Whether `path` is a directory with at least one entry.
pub fn dir_has_entries(path: &Path) -> bool {
	std::fs::read_dir(path)
		.map(|mut it| it.next().is_some())
		.unwrap_or(false)
}

/// Move `staging` into place at `target`, keeping any existing `target` as
/// `<target>.old`. Both must be on the same filesystem (atomic rename).
pub(super) async fn replace_dir(staging: &Path, target: &Path) -> Result<()> {
	use miette::{Context as _, IntoDiagnostic as _};

	if target.exists() {
		let backup = with_extension_suffix(target, "old");
		if backup.exists() {
			tokio::fs::remove_dir_all(&backup)
				.await
				.into_diagnostic()
				.wrap_err_with(|| format!("removing stale {}", backup.display()))?;
		}
		rename_when_free(target, &backup).await.wrap_err_with(|| {
			format!(
				"moving {} aside to {}{}",
				target.display(),
				backup.display(),
				holders::describe_holders(target)
			)
		})?;
	}
	if let Some(parent) = target.parent() {
		tokio::fs::create_dir_all(parent).await.ok();
	}
	rename_when_free(staging, target)
		.await
		.wrap_err_with(|| format!("moving restored data into {}", target.display()))
}

/// How long to keep retrying a rename that fails because the tree is still busy.
/// A service that has just been told to stop lets go of its files in well under
/// a second; past this it's a holder the operator has to clear.
const BUSY_RETRY_FOR: Duration = Duration::from_secs(10);

/// Rename `from` to `to`, riding out a tree that is busy but about to be free.
///
/// Windows reports a directory rename blocked by an open handle or a running
/// image as a flat access denial, and the block is often momentary: the Service
/// Control Manager calls a service stopped before its last child process has
/// finished exiting, and the executables those children ran stay locked a beat
/// longer — for a whole-install postgres restore, inside the very directory
/// being renamed. Other errors (a missing source, a cross-device move) never
/// clear on their own, so they return at once.
async fn rename_when_free(from: &Path, to: &Path) -> Result<()> {
	use miette::IntoDiagnostic as _;

	let deadline = Instant::now() + BUSY_RETRY_FOR;
	let mut backoff = Duration::from_millis(100);
	loop {
		match tokio::fs::rename(from, to).await {
			Ok(()) => return Ok(()),
			Err(err) if is_busy(&err) && Instant::now() + backoff < deadline => {
				debug!("{} is busy ({err}); retrying in {backoff:?}", from.display());
				tokio::time::sleep(backoff).await;
				backoff = (backoff * 2).min(Duration::from_secs(1));
			}
			Err(err) => return Err(err).into_diagnostic(),
		}
	}
}

/// Whether a rename failed for a reason that may clear by itself: on Windows, a
/// sharing violation or the access denial an open handle in the tree produces.
/// Nothing on Unix, where a rename doesn't care who has the files open.
fn is_busy(err: &std::io::Error) -> bool {
	cfg!(windows)
		&& matches!(
			err.raw_os_error(),
			Some(5 /* ERROR_ACCESS_DENIED */ | 32 /* ERROR_SHARING_VIOLATION */)
		)
}

/// `/a/b` + `old` → `/a/b.old`.
pub(super) fn with_extension_suffix(path: &Path, suffix: &str) -> PathBuf {
	let mut name = path.file_name().unwrap_or_default().to_os_string();
	name.push(".");
	name.push(suffix);
	path.with_file_name(name)
}

#[cfg(test)]
mod tests {
	use super::*;

	// On Linux the simple method prepares a kopia-readable *view* (bindfs/copy),
	// which needs a real source, root, and the kopia user — so it's exercised
	// on-host, not here. Off Linux it still snapshots the path in place.
	#[cfg(not(target_os = "linux"))]
	#[tokio::test]
	async fn simple_prepare_returns_its_path_and_no_tags() {
		let method = Method::Simple(SimpleConfig {
			path: Some(PathBuf::from("/data/custom")),
			path_command: None,
		});
		let prepared = method.prepare("custom").await.unwrap();
		assert_eq!(prepared.path, PathBuf::from("/data/custom"));
		assert!(prepared.extra_tags.is_empty());
		assert!(prepared.ignore.is_empty());
		method.cleanup(prepared).await.unwrap();
	}

	#[test]
	fn simple_config_requires_exactly_one_path_form() {
		let fixed = SimpleConfig {
			path: Some(PathBuf::from("/data")),
			path_command: None,
		};
		assert!(fixed.validate("t").is_ok());

		let resolved = SimpleConfig {
			path: None,
			path_command: Some(vec!["/bin/echo".into(), "/data".into()]),
		};
		assert!(resolved.validate("t").is_ok());

		let neither = SimpleConfig {
			path: None,
			path_command: None,
		};
		assert!(format!("{}", neither.validate("t").unwrap_err()).contains("neither"));

		let both = SimpleConfig {
			path: Some(PathBuf::from("/data")),
			path_command: Some(vec!["/bin/echo".into()]),
		};
		assert!(format!("{}", both.validate("t").unwrap_err()).contains("exactly one"));

		let empty = SimpleConfig {
			path: None,
			path_command: Some(vec![]),
		};
		assert!(format!("{}", empty.validate("t").unwrap_err()).contains("empty"));
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn path_command_resolves_trimmed_absolute_output() {
		let config = SimpleConfig {
			path: None,
			path_command: Some(vec!["/bin/echo".into(), "/var/lib/tamanu/blobs".into()]),
		};
		assert_eq!(
			config.resolve_path().await.unwrap(),
			PathBuf::from("/var/lib/tamanu/blobs")
		);
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn path_command_rejects_relative_and_multiline_output() {
		let relative = SimpleConfig {
			path: None,
			path_command: Some(vec!["/bin/echo".into(), "data/blobs".into()]),
		};
		assert!(
			format!("{}", relative.resolve_path().await.unwrap_err()).contains("absolute")
		);

		let multiline = SimpleConfig {
			path: None,
			path_command: Some(vec!["/bin/echo".into(), "/a\n/b".into()]),
		};
		assert!(
			format!("{}", multiline.resolve_path().await.unwrap_err())
				.contains("exactly one line")
		);
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn path_command_failure_is_an_error() {
		let failing = SimpleConfig {
			path: None,
			path_command: Some(vec!["/bin/sh".into(), "-c".into(), "exit 3".into()]),
		};
		assert!(format!("{}", failing.resolve_path().await.unwrap_err()).contains("exited"));
	}

	#[test]
	fn clobber_guard_blocks_occupied_dir_unless_forced() {
		let tmp = tempfile::tempdir().unwrap();
		let occupied = tmp.path().join("data");
		std::fs::create_dir_all(&occupied).unwrap();
		std::fs::write(occupied.join("PG_VERSION"), "16").unwrap();

		assert!(ensure_not_clobbering(&occupied, false).is_err());
		assert!(ensure_not_clobbering(&occupied, true).is_ok());

		// Absent or empty targets are fine without forcing.
		assert!(ensure_not_clobbering(&tmp.path().join("absent"), false).is_ok());
		let empty = tmp.path().join("empty");
		std::fs::create_dir_all(&empty).unwrap();
		assert!(ensure_not_clobbering(&empty, false).is_ok());
	}

	#[test]
	fn extension_suffix_appends() {
		assert_eq!(
			with_extension_suffix(Path::new("/var/lib/postgresql/16/main"), "old"),
			PathBuf::from("/var/lib/postgresql/16/main.old")
		);
	}

	#[test]
	fn only_windows_busy_errors_are_worth_retrying() {
		let denied = std::io::Error::from_raw_os_error(5);
		let missing = std::io::Error::from_raw_os_error(2);
		assert_eq!(is_busy(&denied), cfg!(windows));
		assert!(!is_busy(&missing));
		assert!(!is_busy(&std::io::Error::other("no os code")));
	}

	#[tokio::test]
	async fn a_hopeless_rename_fails_without_burning_the_retry_budget() {
		let tmp = tempfile::tempdir().unwrap();
		let started = Instant::now();
		rename_when_free(&tmp.path().join("absent"), &tmp.path().join("dest"))
			.await
			.unwrap_err();
		assert!(started.elapsed() < BUSY_RETRY_FOR);
	}

	#[tokio::test]
	async fn replace_dir_keeps_old_and_moves_in() {
		let tmp = tempfile::tempdir().unwrap();
		let target = tmp.path().join("data");
		std::fs::create_dir_all(&target).unwrap();
		std::fs::write(target.join("old-marker"), "x").unwrap();
		let staging = tmp.path().join("staging");
		std::fs::create_dir_all(&staging).unwrap();
		std::fs::write(staging.join("new-marker"), "y").unwrap();

		replace_dir(&staging, &target).await.unwrap();

		assert!(target.join("new-marker").exists());
		assert!(!target.join("old-marker").exists());
		assert!(tmp.path().join("data.old").join("old-marker").exists());
	}
}
