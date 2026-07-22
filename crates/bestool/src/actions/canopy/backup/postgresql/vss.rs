//! Crash-consistent Windows VSS shadow-copy snapshot of a postgres cluster.
//!
//! Creates a persistent, client-accessible shadow copy of the volume the data
//! directory lives on via WMI (`Win32_ShadowCopy.Create`), and hands kopia the
//! shadow's `\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopyN` device path
//! directly — no `diskshadow` script, no mount/expose folder. `ClientAccessible`
//! engages no writers, so the shadow is crash-consistent and restores by plain
//! crash recovery, the same clean-restore property as the btrfs/LVM backends.
//!
//! VSS needs Administrator (the daemon runs as a service). This module is only
//! compiled on Windows (the strategy only resolves to VSS there). The WMI
//! orchestration is exercised end-to-end on a real Windows host by CI
//! (`vss / wmi e2e`); the pure path helpers are unit-tested here.

use std::path::{Path, PathBuf};

use miette::{Context as _, IntoDiagnostic as _, Result, bail, miette};
use serde::Deserialize;
use tracing::{info, warn};
use wmi::{Variant, WMIConnection};

use super::resolve::ResolvedCluster;

/// Teardown state for a prepared shadow copy, released by [`teardown`].
#[derive(Debug)]
pub struct Shadow {
	/// The shadow's `{GUID}` id, for deletion.
	id: String,
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

/// Take the shadow copy; returns the kopia source path (the shadow device path
/// plus the data dir's relative path) and the teardown state. The caller must
/// always pass the result to [`teardown`].
pub async fn prepare(resolved: &ResolvedCluster, need: Option<u64>) -> Result<(PathBuf, Shadow)> {
	let root = backup_root(&resolved.data_dir).to_string_lossy().into_owned();
	let volume = volume_of(&root)?.to_owned();
	let rel = relative_to_volume(&root, &volume).to_owned();

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

	info!(%volume, "creating VSS shadow copy via WMI");
	// WMI/COM is thread-affine and `!Send`, so create the shadow on a blocking thread.
	let created = tokio::task::spawn_blocking(move || create_client_accessible(&volume))
		.await
		.into_diagnostic()
		.wrap_err("joining the VSS shadow-create task")??;

	let source = PathBuf::from(format!("{}\\{rel}", created.device));
	info!(shadow = %created.id, source = %source.display(), "VSS shadow ready");
	Ok((source, Shadow { id: created.id }))
}

/// Release a prepared shadow: delete the shadow copy. Best-effort — a cleanup
/// failure is warned, not fatal (the backup itself already succeeded).
pub async fn teardown(shadow: Shadow) -> Result<()> {
	let id = shadow.id;
	match tokio::task::spawn_blocking(move || delete_shadow(&id)).await {
		Ok(Ok(())) => {}
		Ok(Err(err)) => warn!("deleting VSS shadow failed: {err}"),
		Err(err) => warn!("VSS shadow-delete task panicked: {err}"),
	}
	Ok(())
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
	/// Creates a client-accessible shadow of the system drive, reads a marker back
	/// through the shadow's `\\?\GLOBALROOT` device path, and — when `KOPIA_BIN` is
	/// set — has kopia snapshot that device path (the real question: does Go/kopia
	/// read the GLOBALROOT namespace). A drop guard deletes the shadow even if an
	/// assertion panics.
	#[test]
	#[ignore = "needs Windows admin + VSS; run in the `vss / wmi e2e` CI job"]
	fn wmi_shadow_roundtrip() {
		struct ShadowGuard(String);
		impl Drop for ShadowGuard {
			fn drop(&mut self) {
				let _ = delete_shadow(&self.0);
			}
		}

		let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
		let leaf = format!("bestool-vsswmi-{}", std::process::id());
		let dir = PathBuf::from(format!("{drive}\\{leaf}"));
		std::fs::create_dir_all(&dir).expect("create marker dir on the system drive");
		std::fs::write(dir.join("marker.txt"), b"vss-wmi-ok").expect("write marker");

		let created = create_client_accessible(&drive).expect("create shadow via WMI");
		let guard = ShadowGuard(created.id.clone());
		println!("shadow {} at {}", created.id, created.device);

		let shadow_dir = format!("{}\\{leaf}", created.device);
		let content =
			std::fs::read(format!("{shadow_dir}\\marker.txt")).expect("read marker via device path");
		assert_eq!(content, b"vss-wmi-ok", "marker content via shadow device path");

		if let Some(kopia) = std::env::var_os("KOPIA_BIN") {
			kopia_snapshot(Path::new(&kopia), &shadow_dir);
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
