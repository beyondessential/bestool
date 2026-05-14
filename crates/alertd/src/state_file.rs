use std::{
	collections::HashMap,
	io::Write,
	path::{Path, PathBuf},
};

use jiff::Timestamp;
use miette::{IntoDiagnostic, Result, WrapErr};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tracing::{debug, warn};

const STATE_FILE_NAME: &str = "state.json";
const APP_DIR: &str = "bestool-alertd";

/// Persistent per-alert state kept across daemon restarts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedAlertState {
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub triggered_at: Option<Timestamp>,
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub last_sent_at: Option<Timestamp>,
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub last_output: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub paused_until: Option<Timestamp>,
}

/// On-disk shape of the state file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedState {
	pub saved_at: Option<Timestamp>,
	pub alerts: HashMap<PathBuf, PersistedAlertState>,
}

/// Resolve the default state-file path for this platform.
///
/// Mirrors the path-resolution pattern used by `bestool-psql`'s audit DB.
/// Returns `None` only if every fallback fails (e.g. no `HOME`, no
/// `LOCALAPPDATA`); in that case the caller should run without persistence.
pub fn default_state_file_path() -> Option<PathBuf> {
	let base = state_base_dir()?;
	Some(base.join(APP_DIR).join(STATE_FILE_NAME))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn state_base_dir() -> Option<PathBuf> {
	if let Some(dir) = dirs::state_dir() {
		return Some(dir);
	}
	if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
		return Some(PathBuf::from(dir));
	}
	if let Some(home) = std::env::var_os("HOME") {
		return Some(PathBuf::from(home).join(".local").join("state"));
	}
	None
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn state_base_dir() -> Option<PathBuf> {
	if let Some(dir) = dirs::data_local_dir() {
		return Some(dir);
	}
	#[cfg(target_os = "macos")]
	{
		if let Some(home) = std::env::var_os("HOME") {
			return Some(
				PathBuf::from(home)
					.join("Library")
					.join("Application Support"),
			);
		}
	}
	#[cfg(target_os = "windows")]
	{
		if let Some(localappdata) = std::env::var_os("LOCALAPPDATA") {
			return Some(PathBuf::from(localappdata));
		}
	}
	None
}

/// Read and parse the state file.
///
/// If the file is missing, returns an empty state — that's the first-run path.
/// If the file is unreadable or unparsable, logs a warning, deletes the
/// file, and returns an empty state. Persistence is best-effort; a corrupted
/// file should not block the daemon.
pub fn read(path: &Path) -> PersistedState {
	let content = match std::fs::read_to_string(path) {
		Ok(c) => c,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
			debug!(?path, "state file missing, starting fresh");
			return PersistedState::default();
		}
		Err(err) => {
			warn!(?path, "failed to read state file ({err}); discarding");
			let _ = std::fs::remove_file(path);
			return PersistedState::default();
		}
	};

	match serde_json::from_str::<PersistedState>(&content) {
		Ok(state) => {
			debug!(?path, alerts = state.alerts.len(), "loaded state file");
			state
		}
		Err(err) => {
			warn!(?path, "failed to parse state file ({err}); discarding");
			let _ = std::fs::remove_file(path);
			PersistedState::default()
		}
	}
}

/// Atomically write the state to disk.
///
/// Writes to a tempfile in the same directory, then renames into place.
/// Creates the parent directory if missing.
pub fn write(path: &Path, state: &PersistedState) -> Result<()> {
	let parent = path
		.parent()
		.ok_or_else(|| miette::miette!("state file path has no parent directory: {path:?}"))?;

	std::fs::create_dir_all(parent)
		.into_diagnostic()
		.wrap_err_with(|| format!("creating state directory {parent:?}"))?;

	let mut tmp = NamedTempFile::new_in(parent)
		.into_diagnostic()
		.wrap_err_with(|| format!("creating tempfile in {parent:?}"))?;

	let json = serde_json::to_vec_pretty(state)
		.into_diagnostic()
		.wrap_err("serialising state")?;

	tmp.write_all(&json)
		.into_diagnostic()
		.wrap_err("writing state tempfile")?;

	tmp.as_file()
		.sync_all()
		.into_diagnostic()
		.wrap_err("fsyncing state tempfile")?;

	tmp.persist(path)
		.map_err(|err| miette::miette!("renaming state tempfile into place: {err}"))?;

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use tempfile::TempDir;

	#[test]
	fn default_path_resolves_on_test_host() {
		assert!(
			default_state_file_path().is_some(),
			"every test host should have a resolvable state dir"
		);
	}

	#[test]
	fn read_missing_file_returns_empty() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("missing.json");
		let state = read(&path);
		assert!(state.alerts.is_empty());
	}

	#[test]
	fn read_corrupt_file_returns_empty_and_deletes() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("state.json");
		std::fs::write(&path, "{this is not json").unwrap();
		let state = read(&path);
		assert!(state.alerts.is_empty());
		assert!(!path.exists(), "corrupt state file should be deleted");
	}

	#[test]
	fn write_then_read_round_trips() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("subdir").join("state.json");

		let mut alerts = HashMap::new();
		alerts.insert(
			PathBuf::from("/etc/alerts/disk-full.yml"),
			PersistedAlertState {
				triggered_at: Some("2026-05-13T15:00:00Z".parse().unwrap()),
				last_sent_at: Some("2026-05-13T15:00:00Z".parse().unwrap()),
				last_output: Some("rows=[{...}]".into()),
				paused_until: None,
			},
		);
		let state = PersistedState {
			saved_at: Some("2026-05-13T15:00:01Z".parse().unwrap()),
			alerts,
		};

		write(&path, &state).expect("write should succeed");
		assert!(path.exists(), "parent dir should be auto-created");

		let loaded = read(&path);
		assert_eq!(loaded.alerts.len(), 1);
		let entry = &loaded.alerts[&PathBuf::from("/etc/alerts/disk-full.yml")];
		assert!(entry.triggered_at.is_some());
		assert_eq!(entry.last_output.as_deref(), Some("rows=[{...}]"));
		assert!(entry.paused_until.is_none());
	}

	#[test]
	fn write_overwrites_existing_atomically() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("state.json");

		let first = PersistedState::default();
		write(&path, &first).unwrap();

		let mut alerts = HashMap::new();
		alerts.insert(
			PathBuf::from("a.yml"),
			PersistedAlertState {
				triggered_at: Some("2026-05-13T15:00:00Z".parse().unwrap()),
				..Default::default()
			},
		);
		let second = PersistedState {
			saved_at: None,
			alerts,
		};
		write(&path, &second).unwrap();

		let loaded = read(&path);
		assert_eq!(loaded.alerts.len(), 1);
	}
}
