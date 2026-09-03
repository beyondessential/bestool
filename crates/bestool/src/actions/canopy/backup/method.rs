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
use tracing::{debug, info};

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
	/// Set when the capture froze a whole volume rather than just this source, so
	/// anything else on that volume can be read from it at the same instant.
	pub volume: Option<VolumeCapture>,
}

/// A frozen whole volume, readable at a path.
///
/// Only VSS produces one: btrfs and LVM freeze the mount their data sits on, and
/// a path under that mount is not necessarily inside the snapshot, which
/// descends into neither a nested subvolume nor a filesystem mounted within. It
/// exists so a follower whose source sits on the same volume can be read out of
/// the leader's capture instead of taking a second snapshot of data already
/// frozen.
#[derive(Debug, Clone)]
pub struct VolumeCapture {
	/// The volume it covers, as paths on this platform spell it (e.g. `C:`).
	pub volume: String,
	/// Where the volume's root is readable as of the freeze.
	pub root: PathBuf,
	/// The instant the volume froze.
	pub taken_at: Timestamp,
}

impl VolumeCapture {
	/// Where `path` is readable inside this capture, or `None` if it is on
	/// another volume and so isn't in here at all.
	///
	/// The volume is matched case-insensitively: `c:\data` and `C:\data` are the
	/// same place to Windows, and a def spells its path however its author typed
	/// it.
	/// Joined as a Windows path rather than with [`Path::join`], which uses the
	/// host's separator: only VSS produces one of these, so the result is always
	/// consumed on Windows even when the code is built elsewhere.
	pub fn contains(&self, path: &Path) -> Option<PathBuf> {
		let path = path.to_str()?;
		let (head, rest) = path.split_at_checked(self.volume.len())?;
		if !head.eq_ignore_ascii_case(&self.volume) {
			return None;
		}
		let rest = rest.trim_start_matches(['\\', '/']);
		let root = self.root.to_str()?.trim_end_matches('\\');
		Some(PathBuf::from(if rest.is_empty() {
			root.to_owned()
		} else {
			format!("{root}\\{rest}")
		}))
	}
}

/// What [`Method::cleanup`] has to undo for a prepared source.
#[derive(Debug)]
pub(super) enum Teardown {
	/// The simple method's kopia-readable view (bindfs mount or copy).
	Simple(super::simple::Cleanup),
	/// The secret-key method's normalised tree, plus the view made over it.
	SecretKey {
		view: super::simple::Cleanup,
		staged: PathBuf,
	},
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
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleConfig {
	/// The path kopia snapshots.
	pub path: PathBuf,
}

/// `[tamanu_secret_key]` method: capture the key that decrypts
/// `local_system_secrets`, wherever this host keeps it.
///
/// The location is resolved per install shape (a `crypto.keyFile` on bare metal
/// or Windows, podman's secret store in a container), so a def does not name a
/// platform. Every key is optional: a def that just selects the method is the
/// normal case.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TamanuSecretKeyConfig {
	/// Override the resolved location with a key file path.
	#[serde(default)]
	pub path: Option<PathBuf>,
	/// Package whose config names the key (central-server or facility-server);
	/// detected when unset.
	#[serde(default)]
	pub package: Option<String>,
	/// Override the discovered Tamanu install root.
	#[serde(default)]
	pub root: Option<PathBuf>,
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
	TamanuSecretKey(TamanuSecretKeyConfig),
}

impl Method {
	/// The method's name, used in diagnostics.
	pub fn name(&self) -> &'static str {
		match self {
			Method::Simple(_) => "simple",
			Method::Postgresql(_) => "postgresql",
			Method::TamanuSecretKey(_) => "tamanu_secret_key",
		}
	}

	/// Produce the source kopia will snapshot. `backup_type` is the def's label,
	/// used by methods that key stable paths on it (e.g. btrfs mount points).
	/// `within` is a leader's still-live whole-volume capture, when the run is a
	/// follower of one. A source that sits on that volume is read out of it
	/// rather than live, which is both one snapshot instead of two and the only
	/// way the pair describes the same instant.
	pub async fn prepare(&self, backup_type: &str, within: Option<&VolumeCapture>) -> Result<Prepared> {
		match self {
			Method::Simple(config) => {
				let live = config.path.clone();
				let frozen = within.and_then(|capture| capture.contains(&live));
				if let (Some(source), Some(capture)) = (&frozen, within) {
					info!(
						live = %live.display(),
						source = %source.display(),
						"reading this source from the leader's volume capture"
					);
					let (path, cleanup) = super::simple::prepare(source, backup_type).await?;
					return Ok(Prepared {
						path,
						taken_at: Some(capture.taken_at),
						extra_tags: BTreeMap::new(),
						ignore: Vec::new(),
						teardown: Teardown::Simple(cleanup),
						volume: None,
					});
				}
				let (path, cleanup) = super::simple::prepare(&live, backup_type).await?;
				Ok(Prepared {
					path,
					// A live view (bindfs mount or copy) has no point-in-time freeze.
					taken_at: None,
					extra_tags: BTreeMap::new(),
					ignore: Vec::new(),
					teardown: Teardown::Simple(cleanup),
					volume: None,
				})
			}
			Method::Postgresql(config) => super::postgresql::prepare(config, backup_type).await,
			Method::TamanuSecretKey(config) => {
				let location = super::secret_key::location(config).await?;
				let staged = super::secret_key::stage(
					&location,
					backup_type,
					&super::secret_key::stage_parent(),
				)
				.await?;
				let (path, view) = super::simple::prepare(&staged, backup_type).await?;
				Ok(Prepared {
					path,
					taken_at: Some(Timestamp::now()),
					extra_tags: BTreeMap::new(),
					ignore: Vec::new(),
					teardown: Teardown::SecretKey { view, staged },
					volume: None,
				})
			}
		}
	}

	/// Whether this method's capture can be retained as a rollback point.
	///
	/// The simple method prepares a live view of a path rather than a
	/// point-in-time copy — which is why it reports no freeze instant — so there
	/// is nothing to retain that would still describe a moment later. Checked
	/// before a run starts, so a definition that can't be held says so before it
	/// captures anything.
	pub fn supports_hold(&self) -> bool {
		match self {
			Method::Simple(_) => false,
			Method::TamanuSecretKey(_) => false,
			Method::Postgresql(_) => true,
		}
	}

	/// Retain the prepared capture instead of releasing it.
	///
	/// Promotes the capture out of the run-owned names and paths the next run of
	/// this type reuses and reaps, and returns the path it is readable at
	/// afterwards together with what it takes to release it.
	pub(super) async fn hold(
		&self,
		prepared: Prepared,
		id: &str,
	) -> Result<(PathBuf, super::hold::HeldCapture)> {
		let source = prepared.path;
		match prepared.teardown {
			// Unreachable via the commands, which check `supports_hold` before
			// capturing anything; releasing here keeps that a refusal rather than a
			// leak if it ever is reached.
			Teardown::Simple(cleanup) => {
				super::simple::teardown(cleanup).await?;
				bail!(
					"the simple method captures a live view of {}, not a point in time, \
					 so it cannot be held as a rollback point",
					source.display()
				)
			}
			// Unreachable: `supports_hold` is false, checked before capture.
			Teardown::SecretKey { view, staged } => {
				super::simple::teardown(view).await?;
				tokio::fs::remove_dir_all(&staged).await.ok();
				bail!("the tamanu_secret_key method cannot be held as a rollback point")
			}
			Teardown::Btrfs(mounts) => super::postgresql::btrfs::hold(mounts, id, &source).await,
			Teardown::Lvm(snapshot) => super::postgresql::lvm::hold(snapshot, id, &source).await,
			#[cfg(windows)]
			Teardown::Vss(shadow) => super::postgresql::vss::hold(shadow, id, &source).await,
			Teardown::BaseBackup(root) => super::postgresql::basebackup::hold(root, id, &source).await,
		}
	}

	/// Release whatever `prepare` set up (snapshot, mount, staging dir).
	pub async fn cleanup(&self, prepared: Prepared) -> Result<()> {
		match prepared.teardown {
			Teardown::Simple(cleanup) => super::simple::teardown(cleanup).await,
			Teardown::SecretKey { view, staged } => {
				let released = super::simple::teardown(view).await;
				tokio::fs::remove_dir_all(&staged).await.ok();
				released
			}
			Teardown::Btrfs(mounts) => super::postgresql::btrfs::teardown(mounts).await,
			Teardown::Lvm(snapshot) => super::postgresql::lvm::teardown(snapshot).await,
			#[cfg(windows)]
			Teardown::Vss(shadow) => super::postgresql::vss::teardown(shadow).await,
			Teardown::BaseBackup(root) => super::postgresql::basebackup::teardown(root).await,
		}
	}

	/// A staging directory for the restore, colocated with the eventual target's
	/// filesystem so the final move is an atomic rename.
	///
	/// The postgresql method always yields a parent on the data filesystem (see
	/// [`restore_staging_parent`]). The simple method derives it from the target's
	/// parent and only falls back to the temp dir when the target has no parent (a
	/// bare relative path) — an edge that doesn't arise for the postgres restore
	/// that motivated the cross-device guard.
	///
	/// [`restore_staging_parent`]: super::postgresql::resolve::restore_staging_parent
	pub async fn staging_dir(&self, target_override: Option<&Path>, pid: u32) -> Result<PathBuf> {
		let parent = match self {
			Method::Simple(config) => target_override
				.map(Path::to_path_buf)
				.unwrap_or_else(|| config.path.clone())
				.parent()
				.map(Path::to_path_buf)
				.unwrap_or_else(std::env::temp_dir),
			Method::TamanuSecretKey(config) => {
				// A key file is laid back by rename, so stage beside it; a podman
				// secret is piped in, so anywhere works.
				match super::secret_key::location(config).await? {
					bestool_tamanu::secret_key::SecretKeyLocation::KeyFile(path) => path
						.parent()
						.map(Path::to_path_buf)
						.unwrap_or_else(std::env::temp_dir),
					bestool_tamanu::secret_key::SecretKeyLocation::PodmanSecret(_) => {
						std::env::temp_dir()
					}
				}
			}
			Method::Postgresql(config) => super::postgresql::resolve::restore_staging_parent(config),
		};
		Ok(parent.join(format!(".bestool-restore.{pid}")))
	}

	/// Lay a restored snapshot (in `staging`) back down. Method-specific: the
	/// simple method places files at its path; postgresql does the full
	/// stop/swap/start.
	pub async fn restore(&self, staging: &Path, opts: &RestoreOpts) -> Result<()> {
		match self {
			Method::Simple(config) => {
				let target = match &opts.target {
					Some(target) => target.clone(),
					None => config.path.clone(),
				};
				ensure_not_clobbering(&target, opts.clobber)?;
				replace_dir(staging, &target).await
			}
			Method::Postgresql(config) => super::postgresql::restore(config, staging, opts).await,
			Method::TamanuSecretKey(config) => {
				let location = match &opts.target {
					Some(target) => super::secret_key::classify_target(target)?,
					None => super::secret_key::location(config).await?,
				};
				super::secret_key::lay_down(staging, &location, opts.clobber).await
			}
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
/// being renamed. A cross-device move never clears on its own, so it fails at
/// once with the fix (relocate staging); any other error returns immediately.
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
			// A cross-device move can never succeed by retrying: staging and target
			// are on different filesystems. Fail with the fix rather than let the
			// caller's retry loop spin on an unclearable error forever.
			Err(err) if is_cross_device(&err) => {
				bail!(
					"cannot move {} to {}: they are on different filesystems, so the \
					 swap can't be an atomic rename. Set `staging_dir` in the backup \
					 def to a path on the same filesystem as {}, then retry.",
					from.display(),
					to.display(),
					to.display(),
				);
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

/// Whether a rename failed because source and destination are on different
/// filesystems (`EXDEV` on Unix, `ERROR_NOT_SAME_DEVICE` on Windows). This never
/// clears on its own — the staging dir has to be relocated onto the target's
/// filesystem — so the caller stops rather than retries.
fn is_cross_device(err: &std::io::Error) -> bool {
	#[cfg(windows)]
	const CODE: i32 = 17; // ERROR_NOT_SAME_DEVICE
	#[cfg(not(windows))]
	const CODE: i32 = 18; // EXDEV
	err.raw_os_error() == Some(CODE)
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
			path: PathBuf::from("/data/custom"),
		});
		let prepared = method.prepare("custom", None).await.unwrap();
		assert_eq!(prepared.path, PathBuf::from("/data/custom"));
		assert!(prepared.extra_tags.is_empty());
		assert!(prepared.ignore.is_empty());
		method.cleanup(prepared).await.unwrap();
	}

	fn c_volume() -> VolumeCapture {
		VolumeCapture {
			volume: "C:".into(),
			root: PathBuf::from(r"C:\bestool-backup-shadow\tamanu-postgres"),
			taken_at: Timestamp::UNIX_EPOCH,
		}
	}

	#[test]
	fn a_path_on_the_captured_volume_maps_into_the_shadow() {
		assert_eq!(
			c_volume().contains(Path::new(r"C:\Tamanu\blobs")),
			Some(PathBuf::from(
				r"C:\bestool-backup-shadow\tamanu-postgres\Tamanu\blobs"
			))
		);
	}

	#[test]
	fn the_volume_matches_whatever_case_the_def_spells_it() {
		assert_eq!(
			c_volume().contains(Path::new(r"c:\Tamanu\blobs")),
			c_volume().contains(Path::new(r"C:\Tamanu\blobs"))
		);
	}

	#[test]
	fn a_path_on_another_volume_is_not_in_the_capture() {
		// The whole point of the check: D: was never frozen, so reading it out of
		// C:'s shadow would silently snapshot the wrong bytes.
		assert!(c_volume().contains(Path::new(r"D:\Tamanu\blobs")).is_none());
		// A near-miss on the prefix is still another volume.
		assert!(c_volume().contains(Path::new(r"CC:\blobs")).is_none());
		// And a unix path shares no volume notion with a Windows capture at all.
		assert!(c_volume().contains(Path::new("/var/lib/tamanu/blobs")).is_none());
	}

	#[test]
	fn the_volume_root_itself_maps_to_the_shadow_root() {
		assert_eq!(c_volume().contains(Path::new(r"C:\")), Some(c_volume().root));
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

	#[test]
	fn cross_device_is_recognised_and_distinct_from_busy() {
		// The platform's cross-device code (EXDEV=18 on Unix, ERROR_NOT_SAME_DEVICE=17
		// on Windows) is detected; the other platform's code is not.
		#[cfg(not(windows))]
		let (same_dev, other_dev) = (18, 17);
		#[cfg(windows)]
		let (same_dev, other_dev) = (17, 18);
		assert!(is_cross_device(&std::io::Error::from_raw_os_error(same_dev)));
		assert!(!is_cross_device(&std::io::Error::from_raw_os_error(other_dev)));

		// A cross-device failure is never treated as busy — the two branches must not
		// overlap, so the Windows busy-retry keeps handling only 5/32.
		let xdev = std::io::Error::from_raw_os_error(same_dev);
		assert!(!(is_busy(&xdev) && is_cross_device(&xdev)));
		for busy in [5, 32] {
			assert!(!is_cross_device(&std::io::Error::from_raw_os_error(busy)));
		}
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
