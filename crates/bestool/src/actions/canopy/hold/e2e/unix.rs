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

use super::{Backend, clear_records, lifecycle};
use crate::actions::canopy::backup::{
	hold::{HeldCapture, HoldRecord},
	postgresql::lvm,
};

/// Where the fixtures build their storage. Not the temp dir: the cluster is run
/// by a systemd unit, and `/srv` is outside anything a unit's sandboxing hides.
const SCRATCH_BASE: &str = "/srv";

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
		run("truncate", &["-s", "4G", str(&image)]);
		let loopdev = settle(capture("losetup", &["--find", "--show", str(&image)]));

		// No hyphens in the names: device-mapper doubles them in `/dev/mapper`,
		// which the capture reads back through `lvs`.
		let vg = "bestoolholde2e".to_owned();
		run("vgcreate", &[&vg, &loopdev]);
		run(
			"lvcreate",
			&["--type", "thin-pool", "-L", "2G", "-n", "pgpool", &vg],
		);
		run(
			"lvcreate",
			&["--thin", "-V", "3G", "-n", "pgdata", &format!("{vg}/pgpool")],
		);
		let device = format!("/dev/{vg}/pgdata");
		run("mkfs.ext4", &["-q", &device]);

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
			(Storage::Btrfs { mount, .. }, HeldCapture::Btrfs { snapshot_path, .. }) => {
				// The hold's own top-level mount goes with it, so ask the filesystem
				// rather than the path the record names.
				let Some(name) = snapshot_path.file_name().map(|n| n.to_string_lossy().into_owned())
				else {
					return false;
				};
				capture("btrfs", &["subvolume", "list", str(mount)])
					.lines()
					.any(|line| line.split_whitespace().last() == Some(name.as_str()))
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

	async fn store_in_use(&self) -> Option<f64> {
		match &self.storage {
			// Bytes allocated on a filesystem that is ours alone.
			Storage::Btrfs { mount, .. } => {
				let total = fs4::total_space(mount).ok()?;
				let free = fs4::available_space(mount).ok()?;
				Some(total.saturating_sub(free) as f64)
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

	fn released_margin(&self) -> f64 {
		match self.storage {
			// The ballast is 64 MiB; allow generously for allocation granularity
			// and metadata moving under us.
			Storage::Btrfs { .. } => 32.0 * 1024.0 * 1024.0,
			// 64 MiB of a 2 GiB pool is a little over 3%.
			Storage::ThinLvm { .. } => 1.0,
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
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).expect("creating the scratch directory");
	// postgres descends through it to its data directory.
	run("chmod", &["755", str(&dir)]);
	dir
}

/// Unmount everything mounted from a device the teardown is about to remove,
/// deepest mount first so one inside another comes off before its parent.
fn umount_all_from(matches: impl Fn(&str) -> bool) {
	let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else {
		return;
	};
	let mut targets: Vec<&str> = mounts
		.lines()
		.filter_map(|line| {
			let mut fields = line.split_whitespace();
			let source = fields.next()?;
			let target = fields.next()?;
			matches(source).then_some(target)
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
