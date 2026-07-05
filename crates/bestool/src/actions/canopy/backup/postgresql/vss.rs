//! Crash-consistent Windows VSS shadow-copy snapshot of a postgres cluster.
//!
//! Drives `diskshadow` to take a persistent shadow copy of the volume the data
//! directory lives on, exposed read-only at a **stable** folder so kopia's
//! history/dedup attribute to one source. A VSS shadow with no postgres writer
//! is crash-consistent, so it restores by plain crash recovery — no
//! `backup_label`, the same clean-restore property as the btrfs/LVM backends.
//!
//! VSS needs Administrator (the daemon runs as a service). `diskshadow` exists
//! only on Windows; this is only reached when the strategy resolves to VSS
//! (always on Windows). The `diskshadow` orchestration is verified on a real
//! Windows host; the pure helpers (path math, script generation) are unit-tested
//! here (they operate on strings, so the tests are platform-independent).

use std::path::{Path, PathBuf};

use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use tracing::{info, warn};

use super::resolve::ResolvedCluster;

/// The VSS writer/alias name used in the diskshadow script.
const ALIAS: &str = "bestoolpg";

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

/// Teardown state for a prepared shadow copy, released by [`teardown`].
#[derive(Debug)]
pub struct Shadow {
	/// The folder the shadow is exposed at (also the kopia mount root).
	expose_target: String,
	/// The diskshadow metadata `.cab` written for this run.
	metadata: PathBuf,
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

/// The stable folder the shadow is exposed at, per backup type (on the data
/// volume). Fixed across runs so the kopia source path is stable.
fn expose_target_dir(volume: &str, backup_type: &str) -> String {
	format!("{volume}\\bestool-backup-shadow\\{backup_type}")
}

/// The kopia source path within the exposed shadow.
fn kopia_source(expose_target: &str, rel: &str) -> String {
	format!("{expose_target}\\{rel}")
}

/// The diskshadow script that creates and exposes the shadow.
fn create_script(volume: &str, expose_target: &str, metadata: &str) -> String {
	// `persistent` so the shadow outlives the diskshadow session (kopia reads it
	// afterwards); no writer is involved, so it's crash-consistent.
	format!(
		"set context persistent\n\
		 set metadata \"{metadata}\"\n\
		 set verbose on\n\
		 begin backup\n\
		 add volume {volume} alias {ALIAS}\n\
		 create\n\
		 expose %{ALIAS}% \"{expose_target}\"\n\
		 end backup\n"
	)
}

/// The diskshadow script that deletes the shadow exposed at `expose_target`.
fn delete_script(expose_target: &str) -> String {
	format!("delete shadows exposed \"{expose_target}\"\n")
}

/// Take the shadow copy and expose it; returns the kopia source path and the
/// teardown state. Caller must always pass the result to [`teardown`].
pub async fn prepare(resolved: &ResolvedCluster, backup_type: &str) -> Result<(PathBuf, Shadow)> {
	let root = backup_root(&resolved.data_dir).to_string_lossy().into_owned();
	let volume = volume_of(&root)?.to_owned();
	let rel = relative_to_volume(&root, &volume).to_owned();
	let expose_target = expose_target_dir(&volume, backup_type);
	let metadata = std::env::temp_dir().join(format!("bestool-vss-{}.cab", std::process::id()));

	// Sweep a shadow left exposed here by a crashed run before re-creating.
	reap_stale(&expose_target).await;

	info!(volume = %volume, expose_target = %expose_target, "creating VSS shadow copy");
	run_diskshadow(&create_script(&volume, &expose_target, &metadata.to_string_lossy()))
		.await
		.wrap_err("creating VSS shadow copy")?;

	let source = PathBuf::from(kopia_source(&expose_target, &rel));
	Ok((
		source,
		Shadow {
			expose_target,
			metadata,
		},
	))
}

/// Release a prepared shadow: delete the exposed shadow copy and the metadata.
pub async fn teardown(shadow: Shadow) -> Result<()> {
	if let Err(err) = run_diskshadow(&delete_script(&shadow.expose_target)).await {
		warn!("deleting VSS shadow copy failed: {err}");
	}
	let _ = tokio::fs::remove_file(&shadow.metadata).await;
	Ok(())
}

/// Best-effort: delete any shadow still exposed at `expose_target`.
async fn reap_stale(expose_target: &str) {
	let _ = run_diskshadow(&delete_script(expose_target)).await;
}

/// Run a diskshadow script (`diskshadow /s <file>`), erroring on failure.
async fn run_diskshadow(script: &str) -> Result<()> {
	let script_file =
		std::env::temp_dir().join(format!("bestool-diskshadow-{}.txt", std::process::id()));
	tokio::fs::write(&script_file, script)
		.await
		.into_diagnostic()
		.wrap_err("writing diskshadow script")?;

	let output = tokio::process::Command::new("diskshadow")
		.arg("/s")
		.arg(&script_file)
		.stdin(std::process::Stdio::null())
		.output()
		.await
		.into_diagnostic()
		.wrap_err("spawning diskshadow");
	let _ = tokio::fs::remove_file(&script_file).await;

	let output = output?;
	if !output.status.success() {
		bail!(
			"diskshadow failed ({}): {}",
			output.status,
			String::from_utf8_lossy(&output.stdout).trim()
		);
	}
	Ok(())
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
	fn stable_expose_target_and_source() {
		let target = expose_target_dir("C:", "tamanu-postgres");
		assert_eq!(target, "C:\\bestool-backup-shadow\\tamanu-postgres");
		assert_eq!(
			kopia_source(&target, "Tamanu\\data"),
			"C:\\bestool-backup-shadow\\tamanu-postgres\\Tamanu\\data"
		);
	}

	#[test]
	fn create_script_exposes_a_persistent_shadow() {
		let script = create_script("C:", "C:\\shadow\\pg", "C:\\meta.cab");
		assert!(script.contains("set context persistent"));
		assert!(script.contains("add volume C: alias bestoolpg"));
		assert!(script.contains("expose %bestoolpg% \"C:\\shadow\\pg\""));
		assert!(script.contains("set metadata \"C:\\meta.cab\""));
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

	#[test]
	fn delete_script_targets_the_exposed_path() {
		assert_eq!(
			delete_script("C:\\shadow\\pg"),
			"delete shadows exposed \"C:\\shadow\\pg\"\n"
		);
	}
}
