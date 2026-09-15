//! The VSS backend, on the host's own postgres install.
//!
//! A shadow copy is of the whole volume and is made by the platform rather than
//! by us, so there is no storage to build here: the fixture is the server the
//! machine already has, and the def points the capture at its data directory.
//!
//! This backend also carries the case the others cannot express. A shadow copy
//! outlives a reboot but the junction naming it does not: VSS renumbers
//! `HarddiskVolumeShadowCopyN` per boot, so a hold taken before one points at a
//! device that may not exist. That is the case `reattach` exists for. A runner
//! cannot reboot mid-job, so the junction is pointed at a device number VSS has
//! not handed out, which is the state a reboot leaves behind.

use std::{
	path::{Path, PathBuf},
	process::Command,
};

use tempfile::TempDir;

use super::{
	BALLAST, Backend, DropArgs, FROZEN, HoldAction, MARKER, ReattachArgs, assert_marker,
	assert_state, backup, ballast, clear_records, hold, lifecycle, restore_err, sole_hold, write,
};
use crate::actions::canopy::backup::{
	hold::{CaptureState, HeldCapture, HoldRecord},
	postgresql::vss,
};

/// A device number high enough that VSS will not have handed it out, standing in
/// for the renumbering a reboot does.
const BOGUS_DEVICE: &str = r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy999\";

/// The host's postgres install and the def that points a capture at it.
struct Harness {
	backup_type: String,
	backups_dir: TempDir,
	data_dir: PathBuf,
	port: u16,
}

impl Harness {
	fn new(backup_type: &str) -> Self {
		clear_records(backup_type);

		let version = installed_major();
		let data_dir = data_dir(&version);
		assert!(
			data_dir.join("PG_VERSION").is_file(),
			"{} is not a postgres data directory; the VSS lifecycle needs the host's own \
			 cluster (set PGDATA)",
			data_dir.display(),
		);

		let backups_dir = TempDir::new().expect("a directory for the backup def");
		// A literal TOML string: the paths are full of backslashes. No `strategy`
		// override — Windows detects VSS, and the lifecycle asserts the hold ended
		// up on it rather than on the base-backup fallback.
		std::fs::write(
			backups_dir.path().join("def.toml"),
			format!(
				"type = \"{backup_type}\"\n\
				 [postgresql]\n\
				 cluster = \"data\"\n\
				 data_dir = '{}'\n",
				data_dir.display(),
			),
		)
		.expect("writing the backup def");

		Self {
			backup_type: backup_type.to_owned(),
			backups_dir,
			data_dir,
			port: port(),
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
		"vss"
	}

	async fn cluster_is_up(&self) -> bool {
		let bin = crate::find_postgres::find_postgres_bin("pg_isready")
			.expect("pg_isready ships with the postgres install");
		// No credentials: pg_isready asks whether the server answers, not to be let
		// in.
		Command::new(bin)
			.args(["-h", "127.0.0.1", "-p", &self.port.to_string()])
			.status()
			.map(|status| status.success())
			.unwrap_or(false)
	}

	async fn capture_present(&self, record: &HoldRecord) -> bool {
		let HeldCapture::Vss { shadow_id, .. } = &record.capture else {
			panic!("the vss backend held a {} capture", record.capture.backend());
		};
		// The shadow itself, not the junction: a junction outlives the copy behind
		// it, and it is the copy that holds the store.
		vss::held_present(shadow_id).await
	}

	// `store_in_use` is left at `None`. The shadow's store is on the system
	// volume, which the rest of the machine writes to throughout the test, so a
	// byte delta there would be noise rather than evidence. The shadow's absence
	// from `capture_present` is what says its store came back, since deleting a
	// shadow copy is what returns it.
}

impl Drop for Harness {
	fn drop(&mut self) {
		// The cluster is the machine's own, so leave it alone beyond clearing what
		// the lifecycle put in it.
		let _ = std::fs::remove_file(self.data_dir.join(MARKER));
		let _ = std::fs::remove_file(self.data_dir.join(BALLAST));
	}
}

/// The whole lifecycle on a real VSS shadow copy of the host's data volume.
#[tokio::test]
#[ignore = "needs Windows admin, VSS and postgres; run in the `vss / wmi e2e` CI job"]
async fn a_vss_hold_is_taken_used_and_released() {
	lifecycle(&Harness::new("hold-e2e-vss")).await;
}

/// The state a reboot leaves a hold in, and the way back out of it.
///
/// A shadow copy survives the machine restarting; the junction naming it does
/// not, because the device number it substitutes is handed out afresh each boot.
/// A hold in that state is detached, not gone — it must say so, refuse to be
/// restored from, and come back when reattached.
#[tokio::test]
#[ignore = "needs Windows admin, VSS and postgres; run in the `vss / wmi e2e` CI job"]
async fn a_hold_whose_junction_went_stale_is_reattached() {
	let backend = Harness::new("hold-e2e-vss-reboot");
	let data_dir = backend.data_dir().to_path_buf();

	write(&data_dir.join(MARKER), FROZEN);
	write(&data_dir.join(BALLAST), &ballast(1));

	backup(backend.backup_type(), backend.backups_dir())
		.await
		.expect("taking a capture-only hold");
	let held = sole_hold(backend.backup_type()).await;
	assert_eq!(held.capture.backend(), "vss");
	assert_state(&held, CaptureState::Present, "just after it was taken").await;

	let HeldCapture::Vss { junction, .. } = &held.capture else {
		panic!("the vss backend held a {} capture", held.capture.backend());
	};
	point_at_a_bogus_device(junction);

	// The copy is intact and nothing is serving it: that is detached, and telling
	// it from gone is what makes it actionable.
	assert_state(
		&held,
		CaptureState::Detached,
		"with its junction pointing at a device VSS has not handed out",
	)
	.await;

	// A restore must refuse rather than lay an empty tree over the cluster, and
	// name the state so the operator knows there is a way back.
	let err = restore_err(&held, backend.backups_dir()).await;
	assert!(
		err.contains("not mounted") && err.contains("reattach"),
		"a restore from a detached hold has to name the state and the remedy: {err}",
	);

	// Rebuilt from the shadow id, which a reboot does not change.
	hold(HoldAction::Reattach(ReattachArgs { id: held.id.clone() }))
		.await
		.expect("reattaching a hold whose junction went stale");
	assert_state(&held, CaptureState::Present, "after reattaching it").await;
	assert_marker(&held.source, FROZEN, "the reattached capture");

	hold(HoldAction::Drop(DropArgs { id: held.id.clone() }))
		.await
		.expect("dropping the reattached hold");
	assert!(
		!backend.capture_present(&held).await,
		"the shadow copy behind hold {} outlived the drop",
		held.id,
	);
}

/// Repoint a hold's junction at a device number VSS has not handed out: the
/// shadow copy is untouched, and nothing resolves through the junction — the
/// state a reboot leaves a hold taken before it in.
fn point_at_a_bogus_device(junction: &Path) {
	// `junction::create` needs the link path free; removing a junction unmounts
	// it and never touches the copy behind it.
	let _ = std::fs::remove_dir(junction);
	junction::create(BOGUS_DEVICE, junction)
		.expect("repointing the junction at a device number VSS has not handed out");
}

/// The highest installed server major, as the restore's own version check
/// resolves them.
fn installed_major() -> String {
	let base = std::env::var_os("ProgramFiles")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
		.join("PostgreSQL");
	std::fs::read_dir(&base)
		.unwrap_or_else(|err| panic!("reading {}: {err}", base.display()))
		.flatten()
		.filter_map(|entry| entry.file_name().into_string().ok())
		.filter_map(|name| name.parse::<u32>().ok())
		.max()
		.unwrap_or_else(|| panic!("no versioned postgres install under {}", base.display()))
		.to_string()
}

/// The host cluster's data directory. `PGDATA` where the install sets it (it is
/// not always under the install root), else the EDB default beside the binaries.
fn data_dir(version: &str) -> PathBuf {
	if let Some(pgdata) = std::env::var_os("PGDATA") {
		return PathBuf::from(pgdata);
	}
	std::env::var_os("ProgramFiles")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
		.join("PostgreSQL")
		.join(version)
		.join("data")
}

fn port() -> u16 {
	std::env::var("PGPORT")
		.ok()
		.and_then(|raw| raw.trim().parse().ok())
		.unwrap_or(5432)
}
