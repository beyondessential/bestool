use std::{
	mem::replace,
	path::{Path, PathBuf},
	sync::{Arc, Mutex},
};

use miette::{IntoDiagnostic, Result};
use redb::{
	Database, ReadableDatabase, ReadableTable, ReadableTableMetadata as _,
	backends::InMemoryBackend,
};
use tracing::{debug, error, info, instrument, warn};

use crate::repl::ReplState;

use super::multi_process::{WorkingDatabase, spawn_sync_task, sync_to_main};

impl super::Audit {
	/// Open or create an audit database at the given path
	pub fn open(path: impl AsRef<Path>, repl_state: Arc<Mutex<ReplState>>) -> Result<Self> {
		let path = path.as_ref();
		Self::open_internal(path, repl_state, true)
	}

	/// Open an audit database without importing from ~/.psql_history
	///
	/// This is useful for tests where we want a clean database.
	#[cfg(test)]
	pub fn open_empty(path: impl AsRef<Path>) -> Result<Self> {
		let path = path.as_ref();
		Self::open_internal(path, Arc::new(Mutex::new(ReplState::new())), false)
	}

	#[instrument(level = "debug")]
	fn open_internal(
		dir: &Path,
		repl_state: Arc<Mutex<ReplState>>,
		new_db_import_psql_history: bool,
	) -> Result<Self> {
		// Validate that the path is a directory
		if dir.exists() && !dir.is_dir() {
			return Err(miette::miette!(
				"audit path must be a directory, not a file: {:?}",
				dir
			));
		}

		// Create directory if it doesn't exist
		if !dir.exists() {
			std::fs::create_dir_all(dir).into_diagnostic()?;
		}

		// Migrate old database if needed
		Self::migrate_old_database(dir)?;

		let main_path = Self::main_db_path(dir);
		let is_new_main_db = !main_path.exists();

		// Step 1: If main database doesn't exist, create it and import psql history
		if is_new_main_db {
			debug!(?main_path, "creating new main audit database");
			let db = Database::create(&main_path).into_diagnostic()?;
			let db = Arc::new(db);

			let temp_audit = Self {
				db: db.clone(),
				repl_state: repl_state.clone(),
				working_info: None,
			};

			ensure_index_table(&temp_audit)?;

			if new_db_import_psql_history {
				let index_len = temp_audit.hist_index_len()?;
				if index_len == 0
					&& let Err(e) = import_psql_history(&temp_audit)
				{
					debug!("could not import psql history: {e}");
				}
			}

			drop(temp_audit);
			drop(db);
		}

		// Step 2: Copy main database to working file
		let (working_path, uuid) = WorkingDatabase::generate_path(&main_path);
		let working_info = Arc::new(WorkingDatabase::new(
			main_path.clone(),
			working_path.clone(),
			uuid,
		));

		// Try to open main database and copy it
		let copy_result = (|| -> Result<()> {
			// Open main database read-only with retries
			let main_db =
				working_info.open_main_readonly(super::multi_process::MAX_STARTUP_RETRIES, true)?;

			// Copy to working file
			working_info.copy_from_main()?;

			// Close main database
			drop(main_db);

			Ok(())
		})();

		// Step 3: Open working database read-write
		let working_db = match copy_result {
			Ok(()) => {
				// Successfully copied, open the working database
				Database::create(&working_path).into_diagnostic()?
			}
			Err(e) => {
				// Failed to copy after retries, create empty working database and warn
				warn!(
					"could not access main audit database after {} attempts, creating empty working database: {}",
					super::multi_process::MAX_STARTUP_RETRIES,
					e
				);
				Database::create(&working_path).into_diagnostic()?
			}
		};

		let working_db = Arc::new(working_db);

		let audit = Self {
			db: working_db.clone(),
			repl_state,
			working_info: Some(working_info.clone()),
		};

		ensure_index_table(&audit)?;
		cull_db_if_oversize(&audit, &working_path)?;

		// Step 4: Spawn background sync task
		spawn_sync_task(working_db, working_info.clone());

		// Step 5: Spawn orphan database recovery task
		WorkingDatabase::spawn_orphan_recovery(main_path);

		debug!(?audit.db, ?working_path, "opened working audit database");

		Ok(audit)
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

	/// Sync and cleanup on shutdown
	pub(crate) fn shutdown(&self) -> Result<()> {
		if let Some(working_info) = &self.working_info {
			// Signal shutdown to background task
			working_info
				.shutdown
				.store(true, std::sync::atomic::Ordering::Relaxed);

			// Give background task a moment to stop
			std::thread::sleep(std::time::Duration::from_millis(100));

			// Perform final sync
			debug!("performing final sync before shutdown");
			match sync_to_main(&self.db, working_info) {
				Ok(()) => {
					debug!("final sync completed successfully");
					// Delete working database on successful sync
					if let Err(e) = working_info.delete() {
						warn!("failed to delete working database: {}", e);
					}
				}
				Err(e) => {
					error!(
						"failed to sync to main database on shutdown after {} attempts: {}",
						super::multi_process::MAX_EXIT_RETRIES,
						e
					);
					// Mark as orphaned for recovery by next instance
					// (database will be closed when Arc is dropped at end of scope)
					if let Err(e) = working_info.mark_as_orphaned() {
						error!("failed to mark working database as orphaned: {}", e);
					} else {
						info!("working database marked as orphaned for recovery");
					}
				}
			}
		}
		Ok(())
	}

	/// Get the default audit database directory
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
		Ok(history_dir)
	}

	/// Get the main database file path from a directory
	pub fn main_db_path(dir: &Path) -> PathBuf {
		dir.join("audit-main.redb")
	}

	/// Migrate old history.redb to audit-main.redb if it exists
	fn migrate_old_database(dir: &Path) -> Result<()> {
		let old_path = dir.join("history.redb");
		let new_path = Self::main_db_path(dir);

		if old_path.exists() && !new_path.exists() {
			// Try to open the old database exclusively to check if it's in use
			match Database::create(&old_path) {
				Ok(_db) => {
					// Successfully opened exclusively, safe to migrate
					drop(_db);
					info!("migrating audit database from history.redb to audit-main.redb");
					std::fs::rename(&old_path, &new_path).into_diagnostic()?;
				}
				Err(_) => {
					// Database is in use by another process
					return Err(miette::miette!(
						"Cannot migrate audit database: history.redb is currently in use.\n\
						Please close all other bestool-psql instances and try again."
					));
				}
			}
		}

		Ok(())
	}
}

/// Ensure the index table exists and is populated from the history table
#[instrument(level = "trace", skip(audit))]
fn ensure_index_table(audit: &super::Audit) -> Result<()> {
	let db = &audit.db;
	let read_txn = db.begin_read().into_diagnostic()?;

	// Check if index table exists and has entries
	if let Ok(index_table) = read_txn.open_table(super::INDEX_TABLE)
		&& index_table.len().into_diagnostic()? > 0
	{
		// Index table already populated
		return Ok(());
	}

	// Need to build index from history table
	let history_table = match read_txn.open_table(super::HISTORY_TABLE) {
		Ok(table) => table,
		Err(_) => return Ok(()), // No history yet
	};

	let mut timestamps = Vec::new();
	for item in history_table.iter().into_diagnostic()? {
		let (timestamp, _) = item.into_diagnostic()?;
		timestamps.push(timestamp.value());
	}
	drop(history_table);
	drop(read_txn);

	if timestamps.is_empty() {
		return Ok(());
	}

	// Timestamps are already sorted because redb stores them in order
	let write_txn = db.begin_write().into_diagnostic()?;
	{
		let mut index_table = write_txn.open_table(super::INDEX_TABLE).into_diagnostic()?;
		for (idx, timestamp) in timestamps.into_iter().enumerate() {
			index_table
				.insert(idx as u64, timestamp)
				.into_diagnostic()?;
		}
	}
	write_txn.commit().into_diagnostic()?;

	Ok(())
}

#[instrument(level = "trace", skip(audit, path))]
fn cull_db_if_oversize(audit: &super::Audit, path: &Path) -> Result<()> {
	let db = &audit.db;
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
		loop {
			let index_len = audit.hist_index_len()?;
			if index_len == 0 {
				break;
			}

			if let Ok(metadata) = std::fs::metadata(path)
				&& metadata.len() <= TARGET_SIZE
			{
				break;
			}

			let to_remove = CULL_BATCH.min(index_len as usize) as u64;

			// Read timestamps to remove
			let mut old_timestamps = Vec::with_capacity(to_remove as usize);
			for i in 0..to_remove {
				if let Some(timestamp) = audit.hist_index_get(i)? {
					old_timestamps.push(timestamp);
				}
			}

			// Remove from history table
			let write_txn = db.begin_write().into_diagnostic()?;
			{
				let mut history_table = write_txn
					.open_table(super::HISTORY_TABLE)
					.into_diagnostic()?;
				for ts in &old_timestamps {
					history_table.remove(*ts).into_diagnostic()?;
				}
			}
			write_txn.commit().into_diagnostic()?;

			// Rebuild index by removing prefix
			audit.hist_index_remove_prefix(to_remove)?;
		}

		let final_size_mb = std::fs::metadata(path)
			.map(|m| m.len() / (1024 * 1024))
			.unwrap_or(0);
		let final_len = audit.hist_index_len()?;
		debug!(
			size_mb = final_size_mb,
			entries = final_len,
			"culled audit database"
		);
	}

	Ok(())
}

/// Import entries from plain text psql history file (~/.psql_history)
#[instrument(level = "trace", skip(audit))]
fn import_psql_history(audit: &super::Audit) -> Result<()> {
	let db = &audit.db;
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

	let mut timestamp = 0u64;
	let mut count = 0usize;

	let write_txn = db.begin_write().into_diagnostic()?;
	{
		let mut history_table = write_txn
			.open_table(super::HISTORY_TABLE)
			.into_diagnostic()?;

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
			history_table
				.insert(timestamp, json.as_str())
				.into_diagnostic()?;
			timestamp += 1;
			count += 1;
		}
	}
	write_txn.commit().into_diagnostic()?;

	// Build index
	for i in 0..count {
		audit.hist_index_push(i as u64)?;
	}

	info!("imported {} entries from psql history", count);
	Ok(())
}
