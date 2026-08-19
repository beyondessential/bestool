//! Crash-consistent Windows VSS shadow-copy snapshot of a postgres cluster.
//!
//! Creates a persistent, client-accessible shadow copy of the volume the data
//! directory lives on via WMI (`Win32_ShadowCopy.Create`), then mounts it at a
//! **stable** per-backup-type folder (`<vol>\bestool-backup-shadow\<type>`) via a
//! directory junction and hands kopia that path — no `diskshadow` script.
//! `ClientAccessible` engages no writers, so the shadow is crash-consistent and
//! restores by plain crash recovery, the same clean-restore property as the
//! btrfs/LVM backends.
//!
//! The junction keeps the kopia source path fixed across runs even though each
//! shadow's raw device path (`\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopyN`)
//! changes. That stability is what lets kopia's per-source cache skip unchanged
//! files instead of re-reading the whole cluster every run.
//!
//! VSS needs Administrator (the daemon runs as a service). This module is only
//! compiled on Windows (the strategy only resolves to VSS there). The WMI +
//! junction orchestration is exercised end-to-end on a real Windows host by CI
//! (`vss / wmi e2e`); the pure path helpers are unit-tested here.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use miette::{Context as _, IntoDiagnostic as _, Result, bail, miette};
use serde::Deserialize;
use tracing::{info, warn};
use wmi::{Variant, WMIConnection};

use super::{
	super::{hold::HeldCapture, method::VolumeCapture},
	resolve::ResolvedCluster,
};

/// Teardown state for a prepared shadow copy, released by [`teardown`].
#[derive(Debug)]
pub struct Shadow {
	/// The shadow's `{GUID}` id, for deletion.
	id: String,
	/// The junction mounting the shadow, to unmount on teardown.
	junction: PathBuf,
}

/// The directory the backup captures. On the EDB layout the whole server install
/// (`…\PostgreSQL\<version>`, carrying `bin`/`lib`/`share` beside `data`) sits one
/// level above the data dir, so snapshot that instead — a restore then brings the
/// exact matching binaries and is version-independent. Falls back to the data dir
/// alone for a non-EDB layout (no sibling `bin`).
fn backup_root(data_dir: &Path) -> &Path {
	match data_dir.parent() {
		Some(parent) if parent.join("bin").is_dir() => parent,
		_ => data_dir,
	}
}

/// The volume prefix of a Windows path, e.g. `C:` from `C:\Tamanu\data`.
fn volume_of(path: &str) -> Result<&str> {
	let bytes = path.as_bytes();
	if bytes.len() >= 2 && bytes[1] == b':' {
		Ok(&path[..2])
	} else {
		bail!("cannot determine the volume of Windows path {path:?}")
	}
}

/// The path of `data_dir` relative to its volume root (no leading separator),
/// e.g. `Tamanu\data` from `C:\Tamanu\data` on volume `C:`.
fn relative_to_volume<'a>(data_dir: &'a str, volume: &str) -> &'a str {
	data_dir
		.strip_prefix(volume)
		.unwrap_or(data_dir)
		.trim_start_matches(['\\', '/'])
}

/// The stable folder the shadow is mounted at, per backup type (on the data
/// volume). Fixed across runs — matching the pre-WMI diskshadow layout — so the
/// kopia source path is stable and kopia's incremental cache keeps working.
fn expose_target_dir(volume: &str, backup_type: &str) -> String {
	format!("{volume}\\bestool-backup-shadow\\{backup_type}")
}

/// The kopia source path within the mounted shadow.
fn kopia_source(expose_target: &str, rel: &str) -> String {
	format!("{expose_target}\\{rel}")
}

/// Take the shadow copy and mount it at the stable folder; returns the kopia
/// source path and the teardown state. The caller must always pass the result to
/// [`teardown`].
pub async fn prepare(
	resolved: &ResolvedCluster,
	backup_type: &str,
	need: Option<u64>,
) -> Result<(PathBuf, Timestamp, Shadow)> {
	let root = backup_root(&resolved.data_dir).to_string_lossy().into_owned();
	let volume = volume_of(&root)?.to_owned();
	let rel = relative_to_volume(&root, &volume).to_owned();
	let expose_target = expose_target_dir(&volume, backup_type);

	// The shadow's copy-on-write area needs room on its storage volume; if it's
	// nearly full the shadow gets dropped mid-backup, so refuse up front.
	let required = super::space::vss_required_free(need);
	if let Some(free) = super::space::available(Path::new(&root))
		&& free < required
	{
		bail!(
			"volume {volume} has only {} free — a VSS shadow of this cluster needs at least {}; free space on {volume} first",
			super::space::fmt_bytes(free),
			super::space::fmt_bytes(required),
		);
	}

	info!(%volume, %expose_target, "creating VSS shadow copy via WMI");
	// WMI/COM is thread-affine and `!Send`, and the junction is blocking fs work,
	// so do the create + mount on a blocking thread.
	let junction = PathBuf::from(&expose_target);
	let (shadow_id, taken_at) = tokio::task::spawn_blocking({
		let junction = junction.clone();
		move || -> Result<(String, Timestamp)> {
			let created = create_client_accessible(&volume)?;
			// The shadow copy now exists: this is the instant the data froze.
			let taken_at = Timestamp::now();
			if let Err(err) = mount_shadow(&created.device, &junction) {
				// Don't leak the shadow if the mount fails.
				let _ = delete_shadow(&created.id);
				return Err(err);
			}
			Ok((created.id, taken_at))
		}
	})
	.await
	.into_diagnostic()
	.wrap_err("joining the VSS shadow task")??;

	let source = PathBuf::from(kopia_source(&expose_target, &rel));
	info!(shadow = %shadow_id, source = %source.display(), "VSS shadow ready");
	Ok((source, taken_at, Shadow { id: shadow_id, junction }))
}

/// Expose a prepared shadow as a whole-volume capture.
///
/// A shadow copy is of the entire volume and the junction mounts its root, so
/// every path on that volume is inside it, frozen at the same instant. That is
/// what lets a follower on the same volume read from here instead of taking a
/// second shadow of data already captured.
pub fn volume_capture(shadow: &Shadow, taken_at: Timestamp) -> Option<VolumeCapture> {
	let junction = shadow.junction.to_str()?;
	Some(VolumeCapture {
		volume: volume_of(junction).ok()?.to_owned(),
		root: shadow.junction.clone(),
		taken_at,
	})
}

/// Release a prepared shadow: unmount the junction and delete the shadow copy.
/// Best-effort — a cleanup failure is warned, not fatal (the backup itself
/// already succeeded).
pub async fn teardown(shadow: Shadow) -> Result<()> {
	let Shadow { id, junction } = shadow;
	match tokio::task::spawn_blocking(move || {
		// Remove the mount point (the junction, not the shadow contents), then the
		// shadow itself.
		let _ = std::fs::remove_dir(&junction);
		delete_shadow(&id)
	})
	.await
	{
		Ok(Ok(())) => {}
		Ok(Err(err)) => warn!("deleting VSS shadow failed: {err}"),
		Err(err) => warn!("VSS shadow-delete task panicked: {err}"),
	}
	Ok(())
}

/// The folder a held shadow is mounted at. On the capture's own volume, since a
/// junction can't cross volumes, and keyed by hold so two holds of one backup
/// type coexist and neither collides with [`expose_target_dir`].
fn held_expose_target_dir(volume: &str, id: &str) -> String {
	format!("{volume}\\bestool-backup-shadow\\held\\{id}")
}

/// Promote this run's capture to a held one.
///
/// The shadow is persistent already — it survives this process and the machine
/// rebooting — so holding it is a matter of where it is reachable from: the run's
/// junction is the next run's to reuse and gets cleared by it, so the shadow is
/// junctioned at the hold's own folder and the run's junction handed back.
/// Returns the path the held capture is readable at, and what it takes to release
/// it.
pub async fn hold(shadow: Shadow, id: &str, source: &Path) -> Result<(PathBuf, HeldCapture)> {
	let rel = source
		.strip_prefix(&shadow.junction)
		.map_err(|_| {
			miette!(
				"{} is not under the capture's mount {}",
				source.display(),
				shadow.junction.display()
			)
		})?
		.to_path_buf();

	let run_junction = shadow.junction.to_string_lossy().into_owned();
	let volume = volume_of(&run_junction)?.to_owned();
	let held_junction = PathBuf::from(held_expose_target_dir(&volume, id));

	let shadow_id = shadow.id.clone();
	let junction = held_junction.clone();
	tokio::task::spawn_blocking(move || -> Result<()> {
		let con = WMIConnection::new()
			.into_diagnostic()
			.wrap_err("connecting to WMI (ROOT\\CIMV2)")?;
		let device = device_path(&con, &shadow_id)?;
		mount_shadow(&device, &junction)
	})
	.await
	.into_diagnostic()
	.wrap_err("joining the shadow-hold task")??;

	// Hand the run's junction back now the shadow is reachable at the hold's own
	// folder. This unmounts a junction; it never touches the shadow's contents.
	let _ = std::fs::remove_dir(&shadow.junction);

	info!(hold = %id, shadow = %shadow.id, junction = %held_junction.display(), "held VSS shadow copy");
	Ok((
		held_junction.join(rel),
		HeldCapture::Vss {
			shadow_id: shadow.id,
			junction: held_junction,
		},
	))
}

/// Whether the shadow copy behind a held capture still exists.
///
/// Worth asking, and asking often: a shadow has no pin. The copy-on-write store
/// it lives in is bounded and shared with every other shadow on the volume, so
/// writes to that volume during a long hold can push VSS into deleting the
/// oldest shadows to make room — including this one, without warning.
pub async fn held_present(shadow_id: &str) -> bool {
	let shadow_id = shadow_id.to_owned();
	tokio::task::spawn_blocking(move || {
		WMIConnection::new()
			.ok()
			.and_then(|con| device_path(&con, &shadow_id).ok())
			.is_some()
	})
	.await
	.unwrap_or(false)
}

/// Release a capture that was promoted to a hold: the same teardown, rebuilt from
/// the hold's record rather than from the run that took it.
pub async fn release_held(shadow_id: &str, junction: &Path) -> Result<()> {
	teardown(Shadow {
		id: shadow_id.to_owned(),
		junction: junction.to_path_buf(),
	})
	.await
}

/// Mount a shadow's device path at `junction` (a directory junction), creating
/// the parent and clearing any stale mount left by a crashed run first —
/// `junction::create` needs the link path not to exist yet.
fn mount_shadow(device: &str, junction: &Path) -> Result<()> {
	if let Some(parent) = junction.parent() {
		std::fs::create_dir_all(parent)
			.into_diagnostic()
			.wrap_err_with(|| format!("creating {}", parent.display()))?;
	}
	// A leftover junction/dir here makes `junction::create` fail; remove it. On a
	// junction this unmounts (doesn't touch the shadow); best-effort.
	let _ = std::fs::remove_dir(junction);
	junction::create(device, junction)
		.into_diagnostic()
		.wrap_err_with(|| format!("junctioning {} to {device}", junction.display()))
}

/// A freshly-created shadow: its id (for deletion) and device path (for reading).
struct Created {
	id: String,
	device: String,
}

/// Create a persistent, client-accessible (writerless, crash-consistent) shadow
/// of `volume` (e.g. `C:`) via WMI, returning its id and `\\?\GLOBALROOT` device
/// path. Uses the `wmi` crate, which wraps COM internally (so no `unsafe`).
fn create_client_accessible(volume: &str) -> Result<Created> {
	let volume = volume.trim_end_matches(['\\', '/']);
	let con = WMIConnection::new()
		.into_diagnostic()
		.wrap_err("connecting to WMI (ROOT\\CIMV2)")?;

	// Win32_ShadowCopy.Create(Volume, Context); Volume needs a trailing backslash.
	let in_params = con
		.get_object("Win32_ShadowCopy")
		.into_diagnostic()
		.wrap_err("getting the Win32_ShadowCopy class")?
		.get_method("Create")
		.into_diagnostic()
		.wrap_err("getting Win32_ShadowCopy.Create")?
		.ok_or_else(|| miette!("Win32_ShadowCopy has no Create method"))?
		.spawn_instance()
		.into_diagnostic()
		.wrap_err("spawning Create in-parameters")?;
	in_params
		.put_property("Volume", Variant::String(format!("{volume}\\")))
		.into_diagnostic()
		.wrap_err("setting Volume")?;
	in_params
		.put_property("Context", Variant::String("ClientAccessible".to_owned()))
		.into_diagnostic()
		.wrap_err("setting Context")?;

	let out = con
		.exec_method("Win32_ShadowCopy", "Create", Some(&in_params))
		.into_diagnostic()
		.wrap_err("calling Win32_ShadowCopy.Create")?
		.ok_or_else(|| miette!("Create returned no output object"))?;

	let return_value = as_u32(&out.get_property("ReturnValue").into_diagnostic()?);
	if return_value != Some(0) {
		bail!(
			"Win32_ShadowCopy.Create failed with ReturnValue {return_value:?} \
			 (see the Win32_ShadowCopy.Create docs for the meaning)"
		);
	}
	let id = match out.get_property("ShadowID").into_diagnostic()? {
		Variant::String(id) => id,
		other => bail!("Create returned a non-string ShadowID: {other:?}"),
	};
	let device = device_path(&con, &id)?;
	Ok(Created { id, device })
}

/// The `\\?\GLOBALROOT\…` device path of the shadow with `id`.
fn device_path(con: &WMIConnection, id: &str) -> Result<String> {
	#[derive(Deserialize)]
	#[serde(rename = "Win32_ShadowCopy")]
	#[serde(rename_all = "PascalCase")]
	struct Row {
		#[serde(rename = "ID")]
		id: String,
		device_object: String,
	}

	// `ID` is the `{GUID}` string; WQL string literals use single quotes.
	let query = format!("SELECT ID, DeviceObject FROM Win32_ShadowCopy WHERE ID = '{id}'");
	let rows: Vec<Row> = con
		.raw_query(query)
		.into_diagnostic()
		.wrap_err("querying the shadow's DeviceObject")?;
	rows.into_iter()
		.find(|row| row.id.eq_ignore_ascii_case(id))
		.map(|row| row.device_object)
		.ok_or_else(|| miette!("could not find shadow {id} after creating it"))
}

/// Delete a shadow by id via `vssadmin` (Win32_ShadowCopy has no Delete method).
fn delete_shadow(id: &str) -> Result<()> {
	let status = std::process::Command::new("vssadmin")
		.args(["delete", "shadows", &format!("/shadow={id}"), "/quiet"])
		.status()
		.into_diagnostic()
		.wrap_err("running vssadmin delete shadows")?;
	if !status.success() {
		bail!("vssadmin delete shadows /shadow={id} failed ({status})");
	}
	Ok(())
}

/// Coerce a WMI numeric variant (Create's `ReturnValue` is a uint32) to `u32`.
fn as_u32(value: &Variant) -> Option<u32> {
	match value {
		Variant::UI4(n) => Some(*n),
		Variant::UI2(n) => Some(u32::from(*n)),
		Variant::UI1(n) => Some(u32::from(*n)),
		Variant::I4(n) => u32::try_from(*n).ok(),
		Variant::I2(n) => u32::try_from(*n).ok(),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The run's junction is cleared by the next run of that type, so a hold must
	/// not sit on it — and two holds of one type must not sit on each other.
	#[test]
	fn held_junctions_are_keyed_by_hold_and_clear_of_the_run_path() {
		let run = expose_target_dir("C:", "tamanu-postgres");
		let held = held_expose_target_dir("C:", "tamanu-postgres-20260814T054412Z");
		let other = held_expose_target_dir("C:", "tamanu-postgres-20260814T060000Z");

		assert_ne!(run, held);
		assert_ne!(held, other);
		// A hold sits below the run path's parent, not inside the run path itself,
		// so clearing the run's junction can't take a hold with it.
		assert!(!held.starts_with(&format!("{run}\\")));
		assert!(held.starts_with("C:\\bestool-backup-shadow\\held\\"));
	}

	#[test]
	fn held_junctions_stay_on_the_captures_volume() {
		// A junction can't cross volumes, so a shadow of D: is exposed on D:.
		assert!(held_expose_target_dir("D:", "x").starts_with("D:\\"));
	}

	#[test]
	fn volume_and_relative_path() {
		assert_eq!(volume_of("C:\\Tamanu\\data").unwrap(), "C:");
		assert_eq!(volume_of("D:\\pg").unwrap(), "D:");
		assert!(volume_of("\\\\server\\share").is_err());
		assert_eq!(relative_to_volume("C:\\Tamanu\\data", "C:"), "Tamanu\\data");
		assert_eq!(relative_to_volume("C:\\pg", "C:"), "pg");
	}

	#[test]
	fn backup_root_is_the_install_dir_on_the_edb_layout() {
		let tmp = tempfile::tempdir().unwrap();
		let install = tmp.path().join("18");
		let data = install.join("data");
		std::fs::create_dir_all(install.join("bin")).unwrap();
		std::fs::create_dir_all(&data).unwrap();
		// With a sibling `bin`, the whole install is captured…
		assert_eq!(backup_root(&data), install);

		// …without one (non-EDB layout), just the data dir.
		let bare = tmp.path().join("bare").join("data");
		std::fs::create_dir_all(&bare).unwrap();
		assert_eq!(backup_root(&bare), bare);
	}

	/// End-to-end on a real Windows host with VSS + admin (the `vss / wmi e2e` CI
	/// job). Ignored by default because it needs those and isn't hermetic.
	///
	/// Exercises the production path: create a client-accessible shadow of the
	/// system drive, **mount it via a junction** at a stable folder, read a marker
	/// back through the junction, and — when `KOPIA_BIN` is set — have kopia
	/// snapshot the junction path (the real questions: does a junction to a
	/// `\\?\GLOBALROOT` shadow work, and does Go/kopia read through it). A drop
	/// guard unmounts the junction and deletes the shadow even if an assertion
	/// panics.
	#[test]
	#[ignore = "needs Windows admin + VSS; run in the `vss / wmi e2e` CI job"]
	fn wmi_shadow_roundtrip() {
		struct Guard {
			id: String,
			junction: PathBuf,
		}
		impl Drop for Guard {
			fn drop(&mut self) {
				let _ = std::fs::remove_dir(&self.junction);
				let _ = delete_shadow(&self.id);
			}
		}

		let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
		let leaf = format!("bestool-vsswmi-{}", std::process::id());
		let dir = PathBuf::from(format!("{drive}\\{leaf}"));
		std::fs::create_dir_all(&dir).expect("create marker dir on the system drive");
		std::fs::write(dir.join("marker.txt"), b"vss-wmi-ok").expect("write marker");

		let created = create_client_accessible(&drive).expect("create shadow via WMI");
		let mount = PathBuf::from(format!("{drive}\\bestool-vsswmi-mount-{}", std::process::id()));
		mount_shadow(&created.device, &mount).expect("mount shadow via junction");
		let guard = Guard {
			id: created.id.clone(),
			junction: mount.clone(),
		};
		println!("shadow {} at {} mounted at {}", created.id, created.device, mount.display());

		// Read the marker back through the junction (not the raw device path).
		let via = mount.join(&leaf).join("marker.txt");
		let content = std::fs::read(&via).expect("read marker through the junction");
		assert_eq!(content, b"vss-wmi-ok", "marker content via the junction");

		if let Some(kopia) = std::env::var_os("KOPIA_BIN") {
			kopia_snapshot(Path::new(&kopia), &mount.join(&leaf).to_string_lossy());
		} else {
			println!("KOPIA_BIN unset; skipped the kopia snapshot check");
		}

		drop(guard);
		let _ = std::fs::remove_dir_all(&dir);
	}

	/// Create a throwaway filesystem repo and snapshot `source`, asserting success.
	fn kopia_snapshot(kopia: &Path, source: &str) {
		let base = std::env::temp_dir();
		let pid = std::process::id();
		let repo = base.join(format!("bestool-vsswmi-repo-{pid}"));
		let config = base.join(format!("bestool-vsswmi-{pid}.config"));
		let cache = base.join(format!("bestool-vsswmi-cache-{pid}"));
		std::fs::create_dir_all(&repo).unwrap();

		let run = |args: &[&str]| {
			let status = std::process::Command::new(kopia)
				.args(args)
				.arg("--config-file")
				.arg(&config)
				.env("KOPIA_PASSWORD", "probe")
				.env("KOPIA_CACHE_DIRECTORY", &cache)
				.status()
				.expect("run kopia");
			assert!(status.success(), "kopia {args:?} failed ({status})");
		};

		run(&["repository", "create", "filesystem", "--path", &repo.to_string_lossy()]);
		run(&["snapshot", "create", source]);
		println!("kopia snapshotted {source} ok");

		let _ = std::fs::remove_dir_all(&repo);
		let _ = std::fs::remove_dir_all(&cache);
		let _ = std::fs::remove_file(&config);
	}
}
