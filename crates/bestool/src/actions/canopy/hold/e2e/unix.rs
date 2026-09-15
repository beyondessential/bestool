//! The Linux backends: btrfs, thin-LVM, and the base-backup fallback.
//!
//! Each builds its own storage — a loopback btrfs filesystem, a thin pool on a
//! loopback PV, or the machine's own disk — puts a real postgres cluster on it,
//! and hands it to the shared lifecycle. The cluster is made with
//! `pg_createcluster`, which yields the `postgresql@<version>-<name>` unit the
//! restore's lay-down stops and starts, so the restore under test is the whole
//! one rather than a copy into a directory.
//!
//! Every backend's storage is torn down on drop, panic or not, and each one
//! clears the hold records its own backup type left behind on the way in, so a
//! run that panicked mid-lifecycle doesn't wedge the next.

use std::{
	path::{Path, PathBuf},
	process::Command,
};

use tempfile::TempDir;

use super::{BALLAST_BYTES, Backend, clear_records, lifecycle};
use crate::actions::canopy::backup::{
	hold::{HeldCapture, HoldRecord},
	postgresql::lvm,
};

/// Where the fixtures build their storage. Not the temp dir: the cluster is run
/// by a systemd unit, and `/srv` is outside anything a unit's sandboxing hides.
const SCRATCH_BASE: &str = "/srv";

/// The thin pool, sized from the ballast rather than fixed, so the two cannot
/// drift apart.
///
/// One lifecycle puts several ballast-sized allocations in the pool — the one
/// the capture pins, the live one, the restore's staged copy, the tree it
/// displaces — on top of the cluster and its WAL. An ext4 volume mounted without
/// `discard` never hands blocks back, and a loopback pool does not autoextend,
/// so a pool that is merely big enough today would fill and flip the filesystem
/// read-only rather than fail an assertion the moment the ballast grew or the
/// driver gained a step.
const POOL_MIB: usize = BALLAST_BYTES / (1024 * 1024) * 32;

/// The thin volume, deliberately larger than the pool backing it: the fixture is
/// exercising a thin volume, and one that could never overcommit would not be
/// one.
const VOLUME_MIB: usize = POOL_MIB * 3 / 2;

/// The loopback file behind the volume group: the pool plus room for its
/// metadata and LVM's own headers.
const IMAGE_MIB: usize = POOL_MIB * 2;

/// The storage a cluster sits on, and what it takes to tear down.
enum Storage {
	/// A btrfs filesystem in a loopback file, the cluster on a subvolume of it.
	Btrfs { loopdev: String, mount: PathBuf },
	/// A thin logical volume in a pool on a loopback physical volume.
	ThinLvm {
		loopdev: String,
		vg: String,
		mount: PathBuf,
	},
	/// Nothing of its own: the cluster sits on the machine's filesystem, where no
	/// snapshot backend applies and the capture is a streamed base backup.
	BaseBackup,
}

/// One backend's storage, cluster, and def, torn down together.
struct Harness {
	backup_type: String,
	backups_dir: TempDir,
	data_dir: PathBuf,
	version: String,
	cluster: String,
	port: u16,
	scratch: PathBuf,
	storage: Storage,
}

impl Harness {
	/// A cluster on a btrfs subvolume in a loopback image.
	async fn btrfs() -> Self {
		let scratch = scratch("btrfs");
		let image = scratch.join("fs.img");
		run("truncate", &["-s", "6G", str(&image)]);
		run("mkfs.btrfs", &["-q", str(&image)]);
		let loopdev = settle(capture("losetup", &["--find", "--show", str(&image)]));

		// The capture snapshots whatever subvolume the data directory's mountpoint
		// is, and mounts the snapshot by name from the filesystem root, so the
		// cluster has to live on a subvolume rather than on the top level.
		let toplevel = scratch.join("toplevel");
		std::fs::create_dir_all(&toplevel).unwrap();
		run("mount", &["-o", "subvolid=5", "--", &loopdev, str(&toplevel)]);
		run(
			"btrfs",
			&["subvolume", "create", str(&toplevel.join("pgdata"))],
		);
		run("umount", &[str(&toplevel)]);

		let mount = scratch.join("pgdata");
		std::fs::create_dir_all(&mount).unwrap();
		run("mount", &["-o", "subvol=pgdata", "--", &loopdev, str(&mount)]);

		Self::with_cluster(
			"hold-e2e-btrfs",
			"holdbtrfs",
			&mount,
			scratch,
			Storage::Btrfs { loopdev, mount: mount.clone() },
		)
		.await
	}

	/// A cluster on a thin logical volume, in a pool small enough that the
	/// ballast the capture pins is a legible share of it.
	async fn thin_lvm() -> Self {
		let scratch = scratch("lvm");
		let image = scratch.join("pv.img");
		run("truncate", &["-s", &format!("{IMAGE_MIB}m"), str(&image)]);
		let loopdev = settle(capture("losetup", &["--find", "--show", str(&image)]));

		// No hyphens in the names: device-mapper doubles them in `/dev/mapper`,
		// which the capture reads back through `lvs`.
		let vg = "bestoolholde2e".to_owned();
		run("vgcreate", &[&vg, &loopdev]);
		run(
			"lvcreate",
			&[
				"--type",
				"thin-pool",
				"-L",
				&format!("{POOL_MIB}m"),
				"-n",
				"pgpool",
				&vg,
			],
		);
		run(
			"lvcreate",
			&[
				"--thin",
				"-V",
				&format!("{VOLUME_MIB}m"),
				"-n",
				"pgdata",
				&format!("{vg}/pgpool"),
			],
		);
		let device = format!("/dev/{vg}/pgdata");
		// Zero the inode tables now rather than letting the kernel do it in the
		// background after the mount: those writes allocate fresh pool blocks, and
		// they would land in the middle of the measurement the pool is here for.
		run(
			"mkfs.ext4",
			&[
				"-q",
				"-E",
				"lazy_itable_init=0,lazy_journal_init=0,nodiscard",
				&device,
			],
		);

		let mount = scratch.join("pgdata");
		std::fs::create_dir_all(&mount).unwrap();
		run("mount", &["--", &device, str(&mount)]);

		Self::with_cluster(
			"hold-e2e-lvm",
			"holdlvm",
			&mount,
			scratch,
			Storage::ThinLvm {
				loopdev,
				vg,
				mount: mount.clone(),
			},
		)
		.await
	}

	/// A cluster on the machine's own filesystem, where no snapshot backend
	/// applies and the capture falls to `pg_basebackup`.
	async fn base_backup() -> Self {
		let scratch = scratch("basebackup");
		Self::with_cluster(
			"hold-e2e-basebackup",
			"holdbase",
			&scratch,
			scratch.clone(),
			Storage::BaseBackup,
		)
		.await
	}

	/// Put a cluster under `parent`, start it, and write the def that names it.
	async fn with_cluster(
		backup_type: &str,
		cluster: &str,
		parent: &Path,
		scratch: PathBuf,
		storage: Storage,
	) -> Self {
		clear_records(backup_type).await;

		let version = installed_major();
		let data_dir = parent.join(&version).join(cluster);
		std::fs::create_dir_all(data_dir.parent().unwrap()).unwrap();

		// `pg_createcluster` builds both the cluster and the systemd unit the
		// restore's lay-down drives, so the stop/swap/start under test is real.
		run(
			"pg_createcluster",
			&["--datadir", str(&data_dir), &version, cluster],
		);
		run("systemctl", &["start", &unit(&version, cluster)]);
		let port = cluster_port(&version, cluster);

		let backups_dir = TempDir::new().expect("a directory for the backup def");
		// No `strategy` override: the backend is detected from the storage as it is
		// on a real host, and the lifecycle asserts which one the hold ended up on.
		std::fs::write(
			backups_dir.path().join("def.toml"),
			format!(
				"type = \"{backup_type}\"\n\
				 [postgresql]\n\
				 cluster = \"{cluster}\"\n\
				 data_dir = \"{}\"\n\
				 port = {port}\n",
				data_dir.display(),
			),
		)
		.expect("writing the backup def");

		Self {
			backup_type: backup_type.to_owned(),
			backups_dir,
			data_dir,
			version,
			cluster: cluster.to_owned(),
			port,
			scratch,
			storage,
		}
	}
}

impl Backend for Harness {
	fn backup_type(&self) -> &str {
		&self.backup_type
	}

	fn backups_dir(&self) -> &Path {
		self.backups_dir.path()
	}

	fn data_dir(&self) -> &Path {
		&self.data_dir
	}

	fn expected_backend(&self) -> &'static str {
		match self.storage {
			Storage::Btrfs { .. } => "btrfs",
			Storage::ThinLvm { .. } => "lvm",
			Storage::BaseBackup => "basebackup",
		}
	}

	fn exposes_separately(&self) -> bool {
		// A base backup is the directory itself, so nothing exposes it and there
		// is never an exposure to put back.
		!matches!(self.storage, Storage::BaseBackup)
	}

	async fn cluster_is_up(&self) -> bool {
		let bin = crate::find_postgres::find_postgres_bin("pg_isready")
			.expect("pg_isready is installed with the postgres client");
		Command::new(bin)
			.args(["-h", "/var/run/postgresql", "-p", &self.port.to_string()])
			.status()
			.map(|status| status.success())
			.unwrap_or(false)
	}

	async fn capture_present(&self, record: &HoldRecord) -> bool {
		match (&self.storage, &record.capture) {
			(Storage::Btrfs { loopdev, .. }, HeldCapture::Btrfs { snapshot_path, .. }) => {
				let Some(name) = snapshot_path.file_name().map(std::path::Path::new) else {
					return false;
				};
				// Asked by mounting the filesystem's own top level and looking for
				// the subvolume there, rather than by reading a listing. A held
				// snapshot sits beside the cluster's subvolume rather than under it,
				// and `btrfs subvolume list` scopes and formats its output according
				// to where it is run from and which version is installed — so the
				// listing is the wrong instrument for a question that has to have
				// the same answer everywhere. The hold's own top-level mount is gone
				// by the time this is asked, so it gets one of its own.
				let probe = self.scratch.join("toplevel-probe");
				std::fs::create_dir_all(&probe).expect("a mountpoint to probe from");
				run("mount", &["-o", "subvolid=5", "--", loopdev, str(&probe)]);
				let present = probe.join(name).is_dir();
				try_run("umount", &[str(&probe)]);
				present
			}
			(Storage::ThinLvm { .. }, HeldCapture::Lvm { vg, lv, .. }) => {
				lvm::held_present(vg, lv).await
			}
			(Storage::BaseBackup, HeldCapture::BaseBackup { root }) => root.exists(),
			(_, capture) => panic!(
				"the {} backend held a {} capture",
				self.expected_backend(),
				capture.backend()
			),
		}
	}

	fn measures_store(&self) -> bool {
		// The base backup's capture is a tree on the machine's own disk, which the
		// rest of the machine is writing to throughout; its absence is the
		// assertion instead.
		!matches!(self.storage, Storage::BaseBackup)
	}

	async fn store_in_use(&self) -> Option<f64> {
		match &self.storage {
			// The data extents in use, read off the allocator rather than through
			// statvfs.
			//
			// btrfs reports free space net of the chunks it has allocated for
			// metadata, and a metadata chunk is duplicated on a single device — so
			// allocating one moves statvfs by hundreds of megabytes that no amount
			// of freeing data brings back, and a cluster with its restored copy
			// beside it makes enough metadata to allocate one. That swamps the
			// ballast. `Data used` counts the extents themselves and is untouched
			// by it.
			Storage::Btrfs { mount, .. } => {
				data_used(&capture_ok("btrfs", &["filesystem", "df", "--raw", str(mount)])?)
			}
			// The space is in the pool, not in the filesystem on the volume: an
			// unmounted snapshot LV holds pool blocks that `df` never sees.
			Storage::ThinLvm { vg, .. } => {
				capture_ok("lvs", &["--noheadings", "-o", "data_percent", &format!("{vg}/pgpool")])?
					.trim()
					.parse()
					.ok()
			}
			// The held capture is a plain directory on the machine's own disk,
			// which the rest of the machine is writing to throughout. Removing the
			// tree *is* returning its bytes, so its absence is the assertion.
			Storage::BaseBackup => None,
		}
	}

	async fn quiesce_store(&self) {
		// The cluster is the only other writer on this filesystem, and the drop's
		// measurement is about what the drop returns, not about what postgres does
		// alongside it. Everything the cluster had to prove it has proved by here.
		try_run("systemctl", &["stop", &unit(&self.version, &self.cluster)]);
		try_run("sync", &["-f", str(&self.data_dir)]);
	}

	async fn settle_store(&self) {
		if let Storage::Btrfs { mount, .. } = &self.storage {
			// A deleted subvolume's extents are freed by the cleaner thread well
			// after the delete returns. This blocks until it has finished, so the
			// measurement never races it.
			try_run("btrfs", &["subvolume", "sync", str(mount)]);
		}
		try_run("sync", &["-f", str(&self.data_dir)]);
	}

	async fn store_diagnostics(&self) -> String {
		match &self.storage {
			Storage::Btrfs { mount, .. } => format!(
				"btrfs filesystem df:\n{}\nbtrfs filesystem usage:\n{}\nsubvolumes:\n{}",
				capture_ok("btrfs", &["filesystem", "df", str(mount)]).unwrap_or_default(),
				capture_ok("btrfs", &["filesystem", "usage", str(mount)]).unwrap_or_default(),
				capture_ok("btrfs", &["subvolume", "list", str(mount)]).unwrap_or_default(),
			),
			Storage::ThinLvm { vg, .. } => format!(
				"lvs:\n{}",
				capture_ok("lvs", &["-o", "lv_name,lv_size,data_percent", vg]).unwrap_or_default(),
			),
			Storage::BaseBackup => String::new(),
		}
	}

	fn released_margin(&self) -> f64 {
		// Half the ballast either way, which leaves room for allocation
		// granularity and metadata moving under the measurement while still being
		// far more than a drop that returned nothing could produce.
		match self.storage {
			Storage::Btrfs { .. } => BALLAST_BYTES as f64 / 2.0,
			// The pool reports a percentage, so the ballast's share of it is what
			// half a ballast comes to.
			Storage::ThinLvm { .. } => 100.0 / POOL_MIB as f64 * BALLAST_BYTES as f64
				/ (1024.0 * 1024.0)
				/ 2.0,
			Storage::BaseBackup => 0.0,
		}
	}
}

impl Drop for Harness {
	fn drop(&mut self) {
		try_run("pg_dropcluster", &["--stop", &self.version, &self.cluster]);

		// A lifecycle that panicked partway leaves the run's or the hold's mount in
		// place, which keeps the device busy and the teardown below failing.
		match &self.storage {
			Storage::Btrfs { loopdev, mount } => {
				umount_all_from(|source| source == loopdev);
				try_run("umount", &[str(mount)]);
				try_run("losetup", &["-d", loopdev]);
			}
			Storage::ThinLvm { loopdev, vg, mount } => {
				let prefix = format!("/dev/mapper/{vg}-");
				umount_all_from(|source| source.starts_with(&prefix));
				try_run("umount", &[str(mount)]);
				try_run("vgremove", &["-f", vg]);
				try_run("pvremove", &["-f", loopdev]);
				try_run("losetup", &["-d", loopdev]);
			}
			Storage::BaseBackup => {}
		}

		let _ = std::fs::remove_dir_all(&self.scratch);
	}
}

/// The whole lifecycle on a crash-consistent btrfs subvolume snapshot.
#[tokio::test]
#[ignore = "needs root, btrfs-progs and postgres; run in the `btrfs hold / e2e` CI job"]
async fn a_btrfs_hold_is_taken_used_and_released() {
	lifecycle(&Harness::btrfs().await).await;
}

/// The whole lifecycle on a crash-consistent thin-LVM snapshot.
#[tokio::test]
#[ignore = "needs root, lvm2 and postgres; run in the `thin-lvm hold / e2e` CI job"]
async fn a_thin_lvm_hold_is_taken_used_and_released() {
	lifecycle(&Harness::thin_lvm().await).await;
}

/// The whole lifecycle on a streamed base backup: the backend with no privileged
/// storage under it, and the one that exposes nothing separately.
#[tokio::test]
#[ignore = "needs root and postgres; run in the `base backup hold / e2e` CI job"]
async fn a_base_backup_hold_is_taken_used_and_released() {
	lifecycle(&Harness::base_backup().await).await;
}

/// The highest installed server major, resolved the way the restore's own
/// version check resolves them.
fn installed_major() -> String {
	std::fs::read_dir("/usr/lib/postgresql")
		.expect("a postgres server is installed under /usr/lib/postgresql")
		.flatten()
		.filter_map(|entry| entry.file_name().into_string().ok())
		.filter_map(|name| name.parse::<u32>().ok())
		.max()
		.expect("a versioned postgres install under /usr/lib/postgresql")
		.to_string()
}

fn unit(version: &str, cluster: &str) -> String {
	format!("postgresql@{version}-{cluster}")
}

/// The port `pg_createcluster` allocated, for the def to connect on.
fn cluster_port(version: &str, cluster: &str) -> u16 {
	let shown = capture("pg_conftool", &["-s", version, cluster, "show", "port"]);
	let value = shown.rsplit('=').next().unwrap_or(&shown).trim();
	value
		.parse()
		.unwrap_or_else(|err| panic!("parsing the cluster port from {shown:?}: {err}"))
}

/// A fresh directory for one backend's storage.
fn scratch(backend: &str) -> PathBuf {
	let dir = PathBuf::from(SCRATCH_BASE).join(format!("bestool-hold-e2e-{backend}"));
	// Anything a previous run left mounted here comes off first. Removing the
	// tree while a filesystem is still mounted inside it would delete through the
	// mount, into storage this is not entitled to touch.
	let under = format!("{}/", dir.display());
	umount_all_under(|target| target == dir.to_string_lossy() || target.starts_with(&under));
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).expect("creating the scratch directory");
	// postgres descends through it to its data directory.
	run("chmod", &["755", str(&dir)]);
	dir
}

/// Unmount everything mounted from a device the teardown is about to remove,
/// deepest mount first so one inside another comes off before its parent.
fn umount_all_from(matches: impl Fn(&str) -> bool) {
	umount_all(|source, _| matches(source));
}

/// Unmount everything mounted at a path, by where it is mounted rather than what
/// it is mounted from.
fn umount_all_under(matches: impl Fn(&str) -> bool) {
	umount_all(|_, target| matches(target));
}

fn umount_all(matches: impl Fn(&str, &str) -> bool) {
	let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else {
		return;
	};
	let mut targets: Vec<&str> = mounts
		.lines()
		.filter_map(|line| {
			let mut fields = line.split_whitespace();
			let source = fields.next()?;
			let target = fields.next()?;
			matches(source, target).then_some(target)
		})
		.collect();
	targets.sort_by_key(|target| std::cmp::Reverse(target.len()));
	for target in targets {
		try_run("umount", &["-l", target]);
	}
}

/// Wait for udev to publish the symlinks a freshly-attached device gets, which
/// the capture reaches its filesystem through.
fn settle(device: String) -> String {
	try_run("udevadm", &["settle"]);
	device
}

/// The `Data ... used=` byte count from `btrfs filesystem df --raw`.
fn data_used(report: &str) -> Option<f64> {
	report
		.lines()
		.find(|line| line.starts_with("Data"))?
		.rsplit_once("used=")?
		.1
		.trim()
		.parse()
		.ok()
}

fn str(path: &Path) -> &str {
	path.to_str().expect("an ASCII path")
}

fn run(program: &str, args: &[&str]) {
	let output = Command::new(program)
		.args(args)
		.output()
		.unwrap_or_else(|err| panic!("spawning {program}: {err}"));
	assert!(
		output.status.success(),
		"{program} {} failed: {}",
		args.join(" "),
		String::from_utf8_lossy(&output.stderr).trim(),
	);
}

fn try_run(program: &str, args: &[&str]) {
	let _ = Command::new(program).args(args).output();
}

fn capture(program: &str, args: &[&str]) -> String {
	capture_ok(program, args)
		.unwrap_or_else(|| panic!("{program} {} produced nothing usable", args.join(" ")))
}

fn capture_ok(program: &str, args: &[&str]) -> Option<String> {
	let output = Command::new(program).args(args).output().ok()?;
	output
		.status
		.success()
		.then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}


#[cfg(test)]
mod tests {
	use super::*;

	/// The allocator's own report, in the shape `btrfs filesystem df --raw` gives
	/// it. Parsed rather than eyeballed because a misread here would silently make
	/// the release assertion meaningless.
	#[test]
	fn data_used_is_read_off_the_allocator_report() {
		let report = "Data, single: total=8388608, used=67108864\n\
			System, DUP: total=8388608, used=16384\n\
			Metadata, DUP: total=268435456, used=147456\n\
			GlobalReserve, single: total=6029312, used=16384";
		assert_eq!(data_used(report), Some(67108864.0));
	}

	/// Anything else is no reading at all, rather than zero — which would read as
	/// a store that had emptied.
	#[test]
	fn an_unreadable_report_is_not_an_empty_store() {
		assert_eq!(data_used(""), None);
		assert_eq!(data_used("Metadata, DUP: total=1, used=2"), None);
		assert_eq!(data_used("Data, single: total=8388608"), None);
	}
}
