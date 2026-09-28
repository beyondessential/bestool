//! kopia's on-disk cache for the canopy-managed repository: where a connection
//! keeps it, how large it may grow, and sweeping away the caches no connection
//! uses any more.
//!
//! spec: BAK#local-cache

use std::{
	fs, io,
	path::{Path, PathBuf},
	process::Command,
	time::{Duration, SystemTime},
};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use tracing::{debug, info, warn};

/// What a connection is for, which decides how the cache budget is split and
/// which cache it uses.
///
/// spec: BAK#local-cache
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheProfile {
	/// Push-only: takes snapshots, never reads data back.
	///
	/// The data-content cache is only written through on upload and never read
	/// (restores and repository maintenance are the readers, and a device does
	/// neither), so it gets the smaller share. The metadata cache is what makes
	/// an incremental snapshot cheap — it holds the previous snapshot's
	/// directory entries, which is what lets unchanged files be reused without
	/// being read and hashed again — so it gets the bulk.
	Push,
	/// Reads data back out of the repository, so data contents dominate.
	Restore,
}

/// Bounds on kopia's on-disk cache.
///
/// kopia has no single "total cache size" knob: sizes are set per cache, and
/// each has a soft limit (only swept once an entry is older than a minimum age,
/// so it is routinely overshot) and a hard limit (swept regardless of age, and
/// unset by default). Leaving the hard limits unset is how a cache with 5 GB
/// soft limits reaches tens of gigabytes.
///
/// So a budget is expressed here as one number and split into per-cache soft
/// and hard limits by [`CacheProfile`]. The hard limits are what actually bound
/// the disk; the soft limits sit below them so ordinary sweeping keeps the cache
/// off the hard limit in steady state.
///
/// The budget covers the two caches kopia can size — data contents and metadata.
/// The index, blob-list and own-writes caches take no size limit from kopia at
/// all, so total on-disk use is the budget plus those; they are small next to
/// the content cache but not nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheLimits {
	pub content_soft_mb: u64,
	pub content_hard_mb: u64,
	pub metadata_soft_mb: u64,
	pub metadata_hard_mb: u64,
}

/// Share of the cache volume a push-only connection may use.
///
/// A device's caches earn their keep by making the next snapshot cheap, not by
/// being large, and the host's disk is sized for the data it serves rather than
/// for a backup tool's scratch space.
pub const PUSH_CACHE_PERCENT: u64 = 5;

/// Share of the cache volume a restore connection may use.
///
/// A restore reads file data back, so caching it is worth real disk — and a
/// restore is a deliberate, attended operation rather than something running
/// behind a live workload.
pub const RESTORE_CACHE_PERCENT: u64 = 20;

/// Budget used when the volume's size can't be determined.
pub const FALLBACK_CACHE_BUDGET_MB: u64 = 4096;

/// Share of the volume's *free* space the cache may take, whatever the
/// share-of-capacity works out to.
///
/// A percentage of capacity is the wrong bound on a volume that is nearly full:
/// the hosts most at risk of running out of disk are exactly the ones where 5%
/// of capacity is a large fraction of what's left. This caps the budget so the
/// cache can never take more than half of the room remaining.
pub const FREE_SPACE_PERCENT: u64 = 50;

/// Floor on the whole budget, so a small volume doesn't leave kopia with a
/// cache too small to be worth keeping. Applied before the free-space cap,
/// which overrides it: a volume with nothing left gets a small cache, not a
/// cache that fills it.
pub const MIN_CACHE_BUDGET_MB: u64 = 512;

/// Floor for either cache's hard limit, so a small budget can't starve one.
/// Never more than half the budget, so it can't quietly inflate a budget the
/// free-space cap deliberately made small.
const MIN_CACHE_MB: u64 = 128;

/// How long a restore cache is kept after it was last used.
///
/// Long enough that a restore or inspection following another finds it warm;
/// short enough that one attended restore doesn't hold a fifth of the volume
/// for good.
pub const RESTORE_CACHE_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// File in a restore cache whose modification time records its last use. kopia
/// only looks at the subdirectories of a cache, so a file beside them is inert.
const LAST_USED_MARKER: &str = "canopy-last-used";

impl CacheProfile {
	/// Share of the cache volume this profile may use, as a percentage.
	pub fn percent_of_volume(self) -> u64 {
		match self {
			Self::Push => PUSH_CACHE_PERCENT,
			Self::Restore => RESTORE_CACHE_PERCENT,
		}
	}

	/// How the budget divides between data contents and metadata, as the
	/// data-contents percentage.
	fn content_percent(self) -> u64 {
		match self {
			Self::Push => 20,
			Self::Restore => 80,
		}
	}

	fn dir_prefix(self) -> &'static str {
		match self {
			Self::Push => "canopy-push-",
			Self::Restore => "canopy-restore-",
		}
	}

	/// The cache directory under `root` this profile uses for the repository at
	/// `bucket`/`prefix`.
	///
	/// Keyed on the repository alone, so every connection to it lands in the
	/// same directory whatever its transient config: that is what makes the
	/// budget bound the device's total and keeps the cache warm between runs.
	/// Left to itself kopia keys the cache on the config path too, which gives
	/// every per-run config a fresh cache that nothing ever removes.
	///
	/// Push and restore get separate caches: they size and split differently,
	/// and on Linux they run as different users, so a shared one would be swept
	/// back and forth and end up with files the other can't write.
	pub fn cache_dir(self, root: &Path, bucket: &str, prefix: &str) -> PathBuf {
		root.join(format!(
			"{}{}",
			self.dir_prefix(),
			repository_key(bucket, prefix)
		))
	}
}

/// A stable name for the repository at `bucket`/`prefix`: hashed, so the bucket
/// isn't written to the device in the clear.
fn repository_key(bucket: &str, prefix: &str) -> String {
	let mut hash = Sha256::new();
	hash.update(bucket.as_bytes());
	hash.update([0]);
	hash.update(prefix.as_bytes());
	hex::encode(hash.finalize())[..16].to_owned()
}

impl CacheLimits {
	/// Split `total_mb` across the two sizeable caches according to `profile`.
	pub fn new(total_mb: u64, profile: CacheProfile) -> Self {
		let per_cache_floor = MIN_CACHE_MB.min(total_mb / 2);
		let content_hard_mb = (total_mb * profile.content_percent() / 100).max(per_cache_floor);
		let metadata_hard_mb = total_mb
			.saturating_sub(content_hard_mb)
			.max(per_cache_floor);
		Self {
			content_soft_mb: soft_of(content_hard_mb),
			content_hard_mb,
			metadata_soft_mb: soft_of(metadata_hard_mb),
			metadata_hard_mb,
		}
	}

	/// The profile's share of the cache volume, capped at half of what is free
	/// on it, or [`FALLBACK_CACHE_BUDGET_MB`] when the volume can't be measured.
	pub fn for_volume(space: Option<VolumeSpace>, profile: CacheProfile) -> Self {
		let budget = match space {
			Some(space) => (space.total_mb * profile.percent_of_volume() / 100)
				.max(MIN_CACHE_BUDGET_MB)
				.min(space.free_mb * FREE_SPACE_PERCENT / 100),
			None => FALLBACK_CACHE_BUDGET_MB,
		};
		Self::new(budget, profile)
	}

	/// The budget for `profile`: `override_mb` if the caller has one (an
	/// absolute size, so it wins over the share), else the profile's share of
	/// the volume `cache_dir` is on.
	pub fn resolve(profile: CacheProfile, override_mb: Option<u64>, cache_dir: &Path) -> Self {
		if let Some(mb) = override_mb {
			return Self::new(mb, profile);
		}
		let space = volume_space(cache_dir);
		if space.is_none() {
			debug!("could not measure the kopia cache volume; using the fallback budget");
		}
		Self::for_volume(space, profile)
	}
}

/// Capacity and free space, in megabytes, of the volume kopia's cache lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeSpace {
	pub total_mb: u64,
	pub free_mb: u64,
}

/// Measure the volume `path` is on.
///
/// The cache directory itself may not exist yet (a host that has never run a
/// backup), so this walks up to the nearest existing ancestor, which is on the
/// same volume in every layout we create.
fn volume_space(path: &Path) -> Option<VolumeSpace> {
	let mut dir = path.to_path_buf();
	loop {
		if dir.exists() {
			return match (fs4::total_space(&dir), fs4::available_space(&dir)) {
				(Ok(total), Ok(free)) => Some(VolumeSpace {
					total_mb: total / 1_000_000,
					free_mb: free / 1_000_000,
				}),
				(Err(err), _) | (_, Err(err)) => {
					debug!(path = %dir.display(), error = %err, "measuring the kopia cache volume");
					None
				}
			};
		}
		if !dir.pop() {
			return None;
		}
	}
}

/// The home kopia derives its cache and config locations from — the kopia
/// user's where we pin it, and the current user's otherwise.
#[cfg(unix)]
fn kopia_home() -> Option<PathBuf> {
	let system = PathBuf::from(super::LINUX_KOPIA_HOME);
	if system.exists() {
		return Some(system);
	}
	std::env::var_os("HOME").map(PathBuf::from)
}

/// Where canopy connections keep their caches: kopia's own cache root, so the
/// caches it placed there by default are found by the sweep.
#[cfg(unix)]
pub fn cache_root() -> Option<PathBuf> {
	Some(kopia_home()?.join(".cache").join("kopia"))
}

#[cfg(windows)]
pub fn cache_root() -> Option<PathBuf> {
	windows_app_data("LOCALAPPDATA", "Local").map(|dir| dir.join("kopia"))
}

/// The directory kopia's own configs live in, whose caches the sweep must leave
/// alone.
#[cfg(unix)]
pub fn config_root() -> Option<PathBuf> {
	Some(kopia_home()?.join(".config").join("kopia"))
}

#[cfg(windows)]
pub fn config_root() -> Option<PathBuf> {
	windows_app_data("APPDATA", "Roaming").map(|dir| dir.join("kopia"))
}

#[cfg(windows)]
fn windows_app_data(var: &str, under_profile: &str) -> Option<PathBuf> {
	std::env::var_os(var).map(PathBuf::from).or_else(|| {
		std::env::var_os("USERPROFILE")
			.map(|profile| PathBuf::from(profile).join("AppData").join(under_profile))
	})
}

/// Record that the restore cache at `dir` was used now, so the sweep keeps it
/// for another [`RESTORE_CACHE_RETENTION`].
pub fn mark_cache_used(dir: &Path) -> io::Result<()> {
	let marker = fs::File::create(dir.join(LAST_USED_MARKER))?;
	marker.set_modified(SystemTime::now())
}

/// When a restore cache was last used: its marker's mtime, or the directory's
/// own for a cache whose marker was never written or was cleared.
fn last_used(dir: &Path) -> Option<SystemTime> {
	fs::metadata(dir.join(LAST_USED_MARKER))
		.or_else(|_| fs::metadata(dir))
		.and_then(|meta| meta.modified())
		.ok()
}

/// Whether `name` is kopia's default cache naming: the first 16 hex digits of a
/// hash of the repository and the config path.
fn is_default_named(name: &str) -> bool {
	name.len() == 16
		&& name
			.bytes()
			.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The subset of a kopia `repository.config` that names its cache.
#[derive(Deserialize)]
struct LocalConfig {
	#[serde(default)]
	caching: Option<Caching>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Caching {
	#[serde(default)]
	cache_directory: Option<String>,
}

/// The cache directories named by the kopia configs in `config_root`. A
/// relative one is relative to the config's own directory, as kopia reads it.
fn live_cache_dirs(config_root: &Path) -> io::Result<Vec<PathBuf>> {
	let entries = match fs::read_dir(config_root) {
		Ok(entries) => entries,
		Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(err) => return Err(err),
	};
	let mut live = Vec::new();
	for entry in entries {
		let path = entry?.path();
		if path.extension().is_none_or(|ext| ext != "config") || !path.is_file() {
			continue;
		}
		let config: LocalConfig = serde_json::from_slice(&fs::read(&path)?).map_err(|err| {
			io::Error::new(
				io::ErrorKind::InvalidData,
				format!("{}: {err}", path.display()),
			)
		})?;
		if let Some(dir) = config
			.caching
			.and_then(|caching| caching.cache_directory)
			.filter(|dir| !dir.is_empty())
		{
			live.push(config_root.join(dir));
		}
	}
	Ok(live)
}

fn same_dir(a: &Path, b: &Path) -> bool {
	match (a.canonicalize(), b.canonicalize()) {
		(Ok(a), Ok(b)) => a == b,
		_ => a == b,
	}
}

/// The caches under `root` that no connection uses any more, as of `now`.
///
/// - A push cache other than `current_push`: a device backs up to one
///   repository, so any other is one it has moved off.
/// - A restore cache unused for [`RESTORE_CACHE_RETENTION`].
/// - A cache under kopia's default naming. kopia keys those on the config path,
///   so the one a per-run config got is never used again once the config is
///   gone. A kopia config in `config_root` may still point at one (a KopiaUI or
///   system install on the same host), so those are kept, and if the configs
///   can't all be read none of these are touched.
///
/// Everything else under `root` (kopia's logs, say) is left alone.
pub fn stale_caches(
	root: &Path,
	current_push: &Path,
	config_root: Option<&Path>,
	now: SystemTime,
) -> io::Result<Vec<PathBuf>> {
	let entries = match fs::read_dir(root) {
		Ok(entries) => entries,
		Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(err) => return Err(err),
	};
	let live = match config_root.map(live_cache_dirs).transpose() {
		Ok(live) => Some(live.unwrap_or_default()),
		Err(err) => {
			warn!(%err, "could not read the kopia configs; leaving default-named caches in place");
			None
		}
	};
	let push_prefix = CacheProfile::Push.dir_prefix();
	let restore_prefix = CacheProfile::Restore.dir_prefix();

	let mut stale = Vec::new();
	for entry in entries {
		let entry = entry?;
		if !entry.file_type()?.is_dir() {
			continue;
		}
		let path = entry.path();
		let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
			continue;
		};
		let unused = if name.starts_with(push_prefix) {
			Some(name.as_str()) != current_push.file_name().and_then(|n| n.to_str())
		} else if name.starts_with(restore_prefix) {
			last_used(&path)
				.and_then(|used| now.duration_since(used).ok())
				.is_some_and(|idle| idle >= RESTORE_CACHE_RETENTION)
		} else if is_default_named(&name) {
			live.as_ref()
				.is_some_and(|live| !live.iter().any(|dir| same_dir(dir, &path)))
		} else {
			false
		};
		if unused {
			stale.push(path);
		}
	}
	Ok(stale)
}

/// Remove the caches under `root` that no connection uses any more (see
/// [`stale_caches`]), returning the ones removed.
pub fn sweep_caches(root: &Path, current_push: &Path) -> Result<Vec<PathBuf>, String> {
	let stale = stale_caches(
		root,
		current_push,
		config_root().as_deref(),
		SystemTime::now(),
	)
	.map_err(|err| format!("listing {}: {err}", root.display()))?;
	if !stale.is_empty() {
		info!(count = stale.len(), dirs = ?stale, "removing kopia caches no longer in use");
		remove_cache_dirs(&stale)?;
	}
	Ok(stale)
}

/// Remove cache directories as the user that owns kopia's caches.
///
/// Under the daemon we are root without DAC write override, so we can't unlink
/// inside the kopia user's directories. Instead each tree goes to the kopia
/// user first (which also covers root-owned files a restore left), and is then
/// removed as that user.
#[cfg(target_os = "linux")]
fn remove_cache_dirs(dirs: &[PathBuf]) -> Result<(), String> {
	use super::{Elevation, LINUX_KOPIA_USER, canopy_elevation, setpriv_as_kopia, sudo_as_kopia};

	let mut rm = match canopy_elevation() {
		Elevation::Direct => return remove_in_process(dirs),
		Elevation::SetPriv => {
			let mut chown = Command::new("chown");
			chown
				.arg("-R")
				.arg(format!("{LINUX_KOPIA_USER}:{LINUX_KOPIA_USER}"))
				.arg("--")
				.args(dirs);
			run(chown, "chown")?;
			setpriv_as_kopia(Path::new("rm"))
		}
		Elevation::Sudo => sudo_as_kopia(Path::new("rm"), None),
		Elevation::Skip(reason) => return Err(reason),
	};
	rm.args(["-rf", "--"]).args(dirs);
	run(rm, "rm")
}

#[cfg(not(target_os = "linux"))]
fn remove_cache_dirs(dirs: &[PathBuf]) -> Result<(), String> {
	remove_in_process(dirs)
}

fn remove_in_process(dirs: &[PathBuf]) -> Result<(), String> {
	let errors: Vec<String> = dirs
		.iter()
		.filter_map(|dir| {
			fs::remove_dir_all(dir)
				.err()
				.map(|err| format!("{}: {err}", dir.display()))
		})
		.collect();
	if errors.is_empty() {
		Ok(())
	} else {
		Err(errors.join("; "))
	}
}

#[cfg(target_os = "linux")]
fn run(mut cmd: Command, what: &str) -> Result<(), String> {
	let output = cmd
		.output()
		.map_err(|err| format!("could not run {what}: {err}"))?;
	if output.status.success() {
		Ok(())
	} else {
		Err(format!(
			"{what} failed ({}): {}",
			output.status,
			String::from_utf8_lossy(&output.stderr).trim()
		))
	}
}

/// Sweep at 80% of the hard limit, so the hard limit is the backstop rather
/// than the working size.
fn soft_of(hard_mb: u64) -> u64 {
	(hard_mb * 80 / 100).max(1)
}

/// Push the cache-sizing args accepted by `repository connect` and `cache set`.
pub fn args_cache_limits(cmd: &mut Command, limits: &CacheLimits) {
	cmd.arg("--content-cache-size-mb")
		.arg(limits.content_soft_mb.to_string())
		.arg("--content-cache-size-limit-mb")
		.arg(limits.content_hard_mb.to_string())
		.arg("--metadata-cache-size-mb")
		.arg(limits.metadata_soft_mb.to_string())
		.arg("--metadata-cache-size-limit-mb")
		.arg(limits.metadata_hard_mb.to_string());
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn push_profile_favours_metadata_over_data_contents() {
		let limits = CacheLimits::new(4096, CacheProfile::Push);
		assert_eq!(limits.content_hard_mb, 819);
		assert_eq!(limits.metadata_hard_mb, 3277);
		// Soft limits sit below the hard ones, so sweeping keeps the cache off
		// the backstop in steady state.
		assert!(limits.content_soft_mb < limits.content_hard_mb);
		assert!(limits.metadata_soft_mb < limits.metadata_hard_mb);
		// The two sizeable caches together stay within the budget.
		assert!(limits.content_hard_mb + limits.metadata_hard_mb <= 4096);
	}

	#[test]
	fn restore_profile_favours_data_contents() {
		let limits = CacheLimits::new(4096, CacheProfile::Restore);
		assert!(limits.content_hard_mb > limits.metadata_hard_mb);
		assert!(limits.content_hard_mb + limits.metadata_hard_mb <= 4096);
	}

	/// The 130 GB server that prompted the limits: 5% to back up, 20% to
	/// restore, on a volume with room to spare.
	#[test]
	fn budgets_are_a_share_of_the_cache_volume() {
		let roomy = VolumeSpace {
			total_mb: 130_000,
			free_mb: 100_000,
		};
		let push = CacheLimits::for_volume(Some(roomy), CacheProfile::Push);
		assert_eq!(push.content_hard_mb + push.metadata_hard_mb, 6500);
		let restore = CacheLimits::for_volume(Some(roomy), CacheProfile::Restore);
		assert_eq!(restore.content_hard_mb + restore.metadata_hard_mb, 26_000);
		// A restore gets more room overall, and much more of it for file data.
		assert!(restore.content_hard_mb > 4 * push.content_hard_mb);
	}

	#[test]
	fn a_nearly_full_volume_caps_the_budget_at_half_of_what_is_left() {
		// Same 130 GB volume with 8 GB free: the share of capacity would be
		// 6.5 GB, nearly everything that remains.
		let tight = VolumeSpace {
			total_mb: 130_000,
			free_mb: 8_000,
		};
		let push = CacheLimits::for_volume(Some(tight), CacheProfile::Push);
		assert_eq!(push.content_hard_mb + push.metadata_hard_mb, 4_000);
		let restore = CacheLimits::for_volume(Some(tight), CacheProfile::Restore);
		assert_eq!(restore.content_hard_mb + restore.metadata_hard_mb, 4_000);
	}

	#[test]
	fn a_volume_with_almost_nothing_left_gets_a_small_cache_not_the_floor() {
		// The floor would ask for 512 MB; only 300 MB is free, so the cache
		// takes half of that rather than most of what remains.
		let full = VolumeSpace {
			total_mb: 130_000,
			free_mb: 300,
		};
		let limits = CacheLimits::for_volume(Some(full), CacheProfile::Push);
		assert_eq!(limits.content_hard_mb + limits.metadata_hard_mb, 150);
	}

	#[test]
	fn an_unmeasurable_volume_falls_back_to_a_fixed_budget() {
		let limits = CacheLimits::for_volume(None, CacheProfile::Push);
		assert_eq!(
			limits.content_hard_mb + limits.metadata_hard_mb,
			FALLBACK_CACHE_BUDGET_MB
		);
	}

	#[test]
	fn a_small_volume_still_leaves_both_caches_usable() {
		// 5% of a 4 GB volume is under the floor, and there's room for the
		// floor, so the floor applies.
		let small = VolumeSpace {
			total_mb: 4096,
			free_mb: 3000,
		};
		let limits = CacheLimits::for_volume(Some(small), CacheProfile::Push);
		assert_eq!(
			limits.content_hard_mb + limits.metadata_hard_mb,
			MIN_CACHE_BUDGET_MB
		);
		assert!(limits.content_hard_mb >= MIN_CACHE_MB);
		assert!(limits.metadata_hard_mb >= MIN_CACHE_MB);
		assert!(limits.content_soft_mb >= 1);
		assert!(limits.metadata_soft_mb >= 1);
	}

	/// The whole fix: nothing per-run goes into the name, so every run of a
	/// repository's connections shares one cache.
	#[test]
	fn a_repository_has_one_cache_per_profile_whatever_the_run() {
		let root = Path::new("/var/lib/kopia/.cache/kopia");
		let first = CacheProfile::Push.cache_dir(root, "bucket", "servers/abc/");
		let again = CacheProfile::Push.cache_dir(root, "bucket", "servers/abc/");
		assert_eq!(first, again);
		assert_eq!(first.parent(), Some(root));

		let restore = CacheProfile::Restore.cache_dir(root, "bucket", "servers/abc/");
		assert_ne!(first, restore);
		let elsewhere = CacheProfile::Push.cache_dir(root, "bucket", "servers/xyz/");
		assert_ne!(first, elsewhere);
		// Bucket and prefix are hashed apart, not concatenated.
		assert_ne!(
			CacheProfile::Push.cache_dir(root, "ab", "c"),
			CacheProfile::Push.cache_dir(root, "a", "bc")
		);
	}

	#[test]
	fn the_cache_name_does_not_carry_the_bucket() {
		let dir = CacheProfile::Push.cache_dir(Path::new("/c"), "secret-bucket", "p/");
		let name = dir.file_name().unwrap().to_str().unwrap();
		assert!(!name.contains("secret-bucket"), "{name}");
		// Nor could it be mistaken for one of kopia's default-named caches.
		assert!(!is_default_named(name));
	}

	fn set_age(path: &Path, now: SystemTime, age: Duration) {
		fs::File::options()
			.write(true)
			.open(path)
			.unwrap()
			.set_modified(now - age)
			.unwrap();
	}

	fn names(paths: &[PathBuf]) -> Vec<String> {
		let mut names: Vec<String> = paths
			.iter()
			.map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
			.collect();
		names.sort();
		names
	}

	#[test]
	fn the_sweep_takes_only_caches_no_connection_uses() {
		let tmp = tempfile::tempdir().unwrap();
		let root = tmp.path().join("cache");
		let config = tmp.path().join("config");
		fs::create_dir_all(&config).unwrap();
		let now = SystemTime::now();

		let current = CacheProfile::Push.cache_dir(&root, "bucket", "now/");
		let moved_off = CacheProfile::Push.cache_dir(&root, "bucket", "before/");
		let fresh_restore = CacheProfile::Restore.cache_dir(&root, "bucket", "now/");
		let old_restore = CacheProfile::Restore.cache_dir(&root, "bucket", "before/");
		for dir in [
			&current,
			&moved_off,
			&fresh_restore,
			&old_restore,
			&root.join("0123456789abcdef"),
			&root.join("fedcba9876543210"),
			&root.join("aaaaaaaaaaaaaaaa"),
			&root.join("cli-logs"),
		] {
			fs::create_dir_all(dir).unwrap();
		}
		fs::write(root.join("abcdefabcdefabcd"), b"a file, not a cache").unwrap();

		mark_cache_used(&fresh_restore).unwrap();
		mark_cache_used(&old_restore).unwrap();
		set_age(
			&old_restore.join(LAST_USED_MARKER),
			now,
			RESTORE_CACHE_RETENTION + Duration::from_secs(60),
		);
		set_age(
			&fresh_restore.join(LAST_USED_MARKER),
			now,
			RESTORE_CACHE_RETENTION - Duration::from_secs(3600),
		);

		// One live config points at a default-named cache by absolute path, one
		// by a path relative to the config directory.
		fs::write(
			config.join("repository.config"),
			serde_json::json!({ "caching": { "cacheDirectory": root.join("fedcba9876543210") } })
				.to_string(),
		)
		.unwrap();
		fs::write(
			config.join("repository-2.config"),
			serde_json::json!({ "caching": { "cacheDirectory": "../cache/aaaaaaaaaaaaaaaa" } })
				.to_string(),
		)
		.unwrap();

		let stale = stale_caches(&root, &current, Some(&config), now).unwrap();
		assert_eq!(
			names(&stale),
			names(&[moved_off, old_restore, root.join("0123456789abcdef")])
		);
	}

	#[test]
	fn unreadable_configs_keep_every_default_named_cache() {
		let tmp = tempfile::tempdir().unwrap();
		let root = tmp.path().join("cache");
		let config = tmp.path().join("config");
		fs::create_dir_all(&config).unwrap();
		fs::write(config.join("repository.config"), b"{ not json").unwrap();

		let current = CacheProfile::Push.cache_dir(&root, "bucket", "now/");
		let moved_off = CacheProfile::Push.cache_dir(&root, "bucket", "before/");
		for dir in [&current, &moved_off, &root.join("0123456789abcdef")] {
			fs::create_dir_all(dir).unwrap();
		}

		let stale = stale_caches(&root, &current, Some(&config), SystemTime::now()).unwrap();
		assert_eq!(stale, vec![moved_off]);
	}

	#[test]
	fn a_host_with_no_cache_yet_has_nothing_to_sweep() {
		let tmp = tempfile::tempdir().unwrap();
		let root = tmp.path().join("never-created");
		let current = CacheProfile::Push.cache_dir(&root, "b", "p");
		assert!(
			stale_caches(&root, &current, None, SystemTime::now())
				.unwrap()
				.is_empty()
		);
	}

	#[test]
	fn a_restore_cache_without_a_marker_ages_by_its_directory() {
		let tmp = tempfile::tempdir().unwrap();
		let root = tmp.path().to_path_buf();
		let current = CacheProfile::Push.cache_dir(&root, "b", "p");
		let restore = CacheProfile::Restore.cache_dir(&root, "b", "p");
		fs::create_dir_all(&restore).unwrap();

		let now = SystemTime::now();
		assert!(stale_caches(&root, &current, None, now).unwrap().is_empty());
		let later = now + RESTORE_CACHE_RETENTION + Duration::from_secs(60);
		assert_eq!(
			stale_caches(&root, &current, None, later).unwrap(),
			vec![restore]
		);
	}

	#[test]
	fn removal_in_process_reports_what_it_could_not_remove() {
		let tmp = tempfile::tempdir().unwrap();
		let there = tmp.path().join("there");
		fs::create_dir_all(there.join("contents")).unwrap();
		fs::write(there.join("contents").join("blob"), b"x").unwrap();
		remove_in_process(std::slice::from_ref(&there)).unwrap();
		assert!(!there.exists());

		let err = remove_in_process(&[tmp.path().join("gone")]).unwrap_err();
		assert!(err.contains("gone"), "{err}");
	}
}
