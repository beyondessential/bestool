use std::{
	mem::replace,
	path::{Path, PathBuf},
	sync::Arc,
};

use miette::{IntoDiagnostic, Result};
use redb::{backends::InMemoryBackend, Database, ReadableDatabase, ReadableTable};
use tracing::{debug, info, instrument, warn};

use crate::repl::ReplState;

impl super::Audit {
	/// Open or create an audit database at the given path
	pub fn open(path: impl AsRef<Path>, repl_state: ReplState) -> Result<Self> {
		let path = path.as_ref();
		Self::open_internal(path, repl_state, true)
	}

	/// Open an audit database without importing from ~/.psql_history
	///
	/// This is useful for tests where we want a clean database.
	#[cfg(test)]
	pub fn open_empty(path: impl AsRef<Path>) -> Result<Self> {
		let path = path.as_ref();
		Self::open_internal(path, ReplState::new(), false)
	}

	#[instrument(level = "debug")]
	fn open_internal(
		path: &Path,
		repl_state: ReplState,
		new_db_import_psql_history: bool,
	) -> Result<Self> {
		let is_new_db = !path.exists();
		debug!(?path, is_new_db, "opening audit database");

		let db = Database::create(path).into_diagnostic()?;

		let mut timestamps = load_timestamps(&db)?;

		// Import plain text psql history if this is a new database
		if new_db_import_psql_history && is_new_db && timestamps.is_empty() {
			if let Err(e) = import_psql_history(&db, &mut timestamps) {
				debug!("could not import psql history: {e}");
			}
		}

		cull_db_if_oversize(&db, path, &mut timestamps)?;

		debug!(?db, "opened audit database");
		let db = Arc::new(db);

		Ok(Self {
			db,
			timestamps,
			repl_state,
		})
	}

	/// Compact the database to reclaim space from deleted entries
	pub fn compact(&mut self) -> Result<()> {
		let db = replace(
			&mut self.db,
			Arc::new(
				Database::builder()
					.create_with_backend(InMemoryBackend::new())
					.unwrap(),
			),
		);
		let mut db =
			Arc::try_unwrap(db).map_err(|_| miette::miette!("Failed to unwrap database"))?;
		db.compact().into_diagnostic()?;
		Ok(())
	}

	/// Get the default audit database path
	pub fn default_path() -> Result<PathBuf> {
		let state_dir = if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
			PathBuf::from(dir)
		} else if let Some(home) = std::env::var_os("HOME") {
			PathBuf::from(home).join(".local").join("state")
		} else if let Some(localappdata) = std::env::var_os("LOCALAPPDATA") {
			// Windows
			PathBuf::from(localappdata)
		} else {
			return Err(miette::miette!("Could not determine state directory"));
		};

		let history_dir = state_dir.join("bestool-psql");
		std::fs::create_dir_all(&history_dir).into_diagnostic()?;
		Ok(history_dir.join("history.redb"))
	}
}

/// Load all timestamps from the database
#[instrument(level = "trace", skip(db))]
fn load_timestamps(db: &Database) -> Result<Vec<u64>> {
	let read_txn = db.begin_read().into_diagnostic()?;

	let table = match read_txn.open_table(super::HISTORY_TABLE) {
		Ok(table) => table,
		Err(_) => return Ok(Vec::new()),
	};

	let mut timestamps = Vec::new();
	for item in table.iter().into_diagnostic()? {
		let (timestamp, _) = item.into_diagnostic()?;
		timestamps.push(timestamp.value());
	}

	Ok(timestamps)
}

#[instrument(level = "trace", skip(db, path, timestamps))]
fn cull_db_if_oversize(db: &Database, path: &Path, timestamps: &mut Vec<u64>) -> Result<()> {
	const MAX_SIZE: u64 = 100 * 1024 * 1024; // 100MB
	const TARGET_SIZE: u64 = 90 * 1024 * 1024; // 90MB
	const CULL_BATCH: usize = 100; // Remove 100 entries at a time

	let Ok(metadata) = std::fs::metadata(path) else {
		return Ok(());
	};

	if metadata.len() > MAX_SIZE {
		let size_mb = metadata.len() / (1024 * 1024);
		info!(size_mb, "audit database is too large, reducing size");

		// Remove oldest entries in batches until we reach target size
		while !timestamps.is_empty() {
			if let Ok(metadata) = std::fs::metadata(path) {
				if metadata.len() <= TARGET_SIZE {
					break;
				}
			}

			let to_remove = CULL_BATCH.min(timestamps.len());
			let old_timestamps: Vec<u64> = timestamps.drain(..to_remove).collect();

			let write_txn = db.begin_write().into_diagnostic()?;
			{
				let mut table = write_txn
					.open_table(super::HISTORY_TABLE)
					.into_diagnostic()?;
				for ts in old_timestamps {
					table.remove(ts).into_diagnostic()?;
				}
			}
			write_txn.commit().into_diagnostic()?;
		}

		let final_size_mb = std::fs::metadata(path)
			.map(|m| m.len() / (1024 * 1024))
			.unwrap_or(0);
		debug!(
			size_mb = final_size_mb,
			entries = timestamps.len(),
			"culled audit database"
		);
	}

	Ok(())
}

/// Import entries from plain text psql history file (~/.psql_history)
#[instrument(level = "trace", skip(db, timestamps))]
fn import_psql_history(db: &Database, timestamps: &mut Vec<u64>) -> Result<()> {
	let psql_history_path = if let Some(home) = std::env::var_os("HOME") {
		PathBuf::from(home).join(".psql_history")
	} else if let Some(userprofile) = std::env::var_os("USERPROFILE") {
		// Windows fallback
		PathBuf::from(userprofile).join(".psql_history")
	} else {
		return Ok(()); // No home directory, skip import
	};

	if !psql_history_path.exists() {
		debug!("no psql history file found at {psql_history_path:?}");
		return Ok(());
	}

	info!("importing psql history from {psql_history_path:?}");

	let content = std::fs::read_to_string(&psql_history_path).into_diagnostic()?;
	let lines: Vec<&str> = content.lines().collect();

	if lines.is_empty() {
		return Ok(());
	}

	let write_txn = db.begin_write().into_diagnostic()?;
	{
		let mut table = write_txn
			.open_table(super::HISTORY_TABLE)
			.into_diagnostic()?;
		let mut timestamp = 0u64;

		for line in lines {
			let line = line.trim();
			if line.is_empty() {
				continue;
			}

			// Create entry with default values
			let entry = super::AuditEntry {
				query: line.to_string(),
				db_user: String::new(),
				sys_user: String::new(),
				writemode: true,
				tailscale: Vec::new(),
				ots: None,
			};

			let json = serde_json::to_string(&entry).into_diagnostic()?;
			table.insert(timestamp, json.as_str()).into_diagnostic()?;
			timestamps.push(timestamp);
			timestamp += 1;
		}
	}
	write_txn.commit().into_diagnostic()?;

	info!("imported {} entries from psql history", timestamps.len());
	Ok(())
}
