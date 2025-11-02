use std::{
	fs,
	path::{Path, PathBuf},
	sync::{
		Arc,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
	thread,
	time::{Duration, SystemTime},
};

use miette::{IntoDiagnostic, Result};
use rand::Rng;
use redb::{Database, ReadableDatabase, ReadableTable};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Interval for periodic sync (60 seconds)
const SYNC_INTERVAL: Duration = Duration::from_secs(60);

/// Maximum retry attempts on startup
pub(super) const MAX_STARTUP_RETRIES: u32 = 5;

/// Maximum retry attempts on exit
pub(super) const MAX_EXIT_RETRIES: u32 = 10;

/// Minimum retry delay (0.1 seconds)
const MIN_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Maximum retry delay (2 seconds)
const MAX_RETRY_DELAY: Duration = Duration::from_secs(2);

/// Orphan file age threshold (1 minute)
const ORPHAN_RECOVERY_THRESHOLD: Duration = Duration::from_secs(60);

/// Pattern prefix for working database files
const WORKING_PREFIX: &str = "audit-working-";

/// Pattern prefix for orphaned database files
const ORPHANED_PREFIX: &str = "audit-orphaned-";

/// Pattern suffix for working/orphaned database files
const DB_SUFFIX: &str = ".redb";

/// Information about our working copy database
#[derive(Debug)]
pub struct WorkingDatabase {
	/// Path to the working database file
	pub path: PathBuf,
	/// Path to the main database file
	pub main_path: PathBuf,
	/// UUID identifier for this working database
	pub uuid: Uuid,
	/// Last timestamp that was synced to main database
	pub last_synced_timestamp: AtomicU64,
	/// Flag to indicate shutdown
	pub shutdown: Arc<AtomicBool>,
}

impl WorkingDatabase {
	/// Generate a new working database path
	pub fn generate_path(main_path: &Path) -> (PathBuf, Uuid) {
		let uuid = Uuid::new_v4();
		let parent = main_path.parent().unwrap();
		let working_name = format!("{}{}{}", WORKING_PREFIX, uuid, DB_SUFFIX);
		(parent.join(working_name), uuid)
	}

	/// Get the orphaned database path for this working database
	pub fn orphaned_path(&self) -> PathBuf {
		let parent = self.path.parent().unwrap();
		let orphaned_name = format!("{}{}{}", ORPHANED_PREFIX, self.uuid, DB_SUFFIX);
		parent.join(orphaned_name)
	}

	/// Create a new WorkingDatabase instance
	pub fn new(main_path: PathBuf, working_path: PathBuf, uuid: Uuid) -> Self {
		Self {
			path: working_path,
			main_path,
			uuid,
			last_synced_timestamp: AtomicU64::new(0),
			shutdown: Arc::new(AtomicBool::new(false)),
		}
	}

	/// Get a random retry delay between MIN_RETRY_DELAY and MAX_RETRY_DELAY
	pub fn random_retry_delay() -> Duration {
		let mut rng = rand::thread_rng();
		let millis = rng.gen_range(MIN_RETRY_DELAY.as_millis()..=MAX_RETRY_DELAY.as_millis());
		Duration::from_millis(millis as u64)
	}

	/// Try to open the main database read-only with retries
	pub fn open_main_readonly(&self, max_retries: u32, log_retries: bool) -> Result<Database> {
		for attempt in 1..=max_retries {
			match Database::open(&self.main_path) {
				Ok(db) => {
					return Ok(db);
				}
				Err(e) => {
					if attempt < max_retries {
						if log_retries && attempt == 1 {
							info!("waiting to access main audit database...");
						}
						let delay = Self::random_retry_delay();
						debug!(
							"failed to open main database (attempt {}/{}), retrying in {:?}: {}",
							attempt, max_retries, delay, e
						);
						thread::sleep(delay);
					} else {
						return Err(e).into_diagnostic();
					}
				}
			}
		}
		unreachable!()
	}

	/// Try to open the main database read-write with retries
	pub fn open_main_readwrite(&self, max_retries: u32, log_retries: bool) -> Result<Database> {
		for attempt in 1..=max_retries {
			match Database::create(&self.main_path) {
				Ok(db) => {
					return Ok(db);
				}
				Err(e) => {
					if attempt < max_retries {
						if log_retries && attempt == 1 {
							info!("waiting to access main audit database...");
						}
						let delay = Self::random_retry_delay();
						debug!(
							"failed to open main database read-write (attempt {}/{}), retrying in {:?}: {}",
							attempt, max_retries, delay, e
						);
						thread::sleep(delay);
					} else {
						return Err(e).into_diagnostic();
					}
				}
			}
		}
		unreachable!()
	}

	/// Copy main database to working database using reflink if possible
	pub fn copy_from_main(&self) -> Result<()> {
		debug!("copying main database to working database: {:?}", self.path);
		reflink_copy::reflink_or_copy(&self.main_path, &self.path).into_diagnostic()?;
		Ok(())
	}

	/// Delete the working database file
	pub fn delete(&self) -> Result<()> {
		debug!("deleting working database: {:?}", self.path);
		if self.path.exists() {
			fs::remove_file(&self.path).into_diagnostic()?;
		}
		Ok(())
	}

	/// Rename working database to orphaned (for failed shutdown)
	pub fn mark_as_orphaned(&self) -> Result<()> {
		let orphaned_path = self.orphaned_path();
		debug!(
			"marking working database as orphaned: {:?} -> {:?}",
			self.path, orphaned_path
		);
		if self.path.exists() {
			fs::rename(&self.path, &orphaned_path).into_diagnostic()?;
		}
		Ok(())
	}

	/// Find all orphaned working databases in the same directory as the main database
	pub fn find_orphan_databases(main_path: &Path) -> Result<Vec<PathBuf>> {
		let parent = main_path.parent().unwrap();
		let mut orphan_dbs = Vec::new();

		let entries = fs::read_dir(parent).into_diagnostic()?;
		for entry in entries {
			let entry = entry.into_diagnostic()?;
			let path = entry.path();

			if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
				// Explicitly orphaned databases (from failed shutdowns)
				if name.starts_with(ORPHANED_PREFIX) && name.ends_with(DB_SUFFIX) {
					orphan_dbs.push(path);
				}
				// Working databases that are old (crash recovery)
				else if name.starts_with(WORKING_PREFIX)
					&& name.ends_with(DB_SUFFIX)
					&& let Ok(metadata) = fs::metadata(&path)
					&& let Ok(modified) = metadata.modified()
					&& let Ok(elapsed) = SystemTime::now().duration_since(modified)
					&& elapsed > ORPHAN_RECOVERY_THRESHOLD
				{
					orphan_dbs.push(path);
				}
			}
		}

		Ok(orphan_dbs)
	}

	/// Try to open a working database exclusively (to check if it's in use)
	pub fn try_open_working_exclusive(path: &Path) -> Result<Database> {
		Database::create(path).into_diagnostic()
	}

	/// Merge working database into main database
	pub fn merge_working_into_main(working_path: &Path, main_path: &Path) -> Result<()> {
		debug!(
			"merging working database {:?} into main database",
			working_path
		);

		// Open working database
		let working_db = Database::open(working_path).into_diagnostic()?;

		// Open main database read-write
		let main_db = Database::create(main_path).into_diagnostic()?;

		// Read all entries from working database
		let working_read_txn = working_db.begin_read().into_diagnostic()?;
		let working_history = match working_read_txn.open_table(super::HISTORY_TABLE) {
			Ok(table) => table,
			Err(_) => {
				debug!("working database has no history table, skipping");
				return Ok(());
			}
		};

		let mut entries_to_copy = Vec::new();
		for item in working_history.iter().into_diagnostic()? {
			let (timestamp, json) = item.into_diagnostic()?;
			entries_to_copy.push((timestamp.value(), json.value().to_string()));
		}
		drop(working_history);
		drop(working_read_txn);
		drop(working_db);

		if entries_to_copy.is_empty() {
			debug!("no entries to merge from working database");
			return Ok(());
		}

		// Write entries to main database
		let main_write_txn = main_db.begin_write().into_diagnostic()?;
		{
			let mut main_history = main_write_txn
				.open_table(super::HISTORY_TABLE)
				.into_diagnostic()?;

			for (timestamp, json) in &entries_to_copy {
				// Use insert which will overwrite if exists, or create if not
				main_history
					.insert(*timestamp, json.as_str())
					.into_diagnostic()?;
			}
		}
		main_write_txn.commit().into_diagnostic()?;

		info!(
			"merged {} entries from working database",
			entries_to_copy.len()
		);
		Ok(())
	}

	/// Spawn background thread to recover orphaned working databases
	pub fn spawn_orphan_recovery(main_path: PathBuf) {
		thread::spawn(move || {
			if let Err(e) = Self::recover_orphan_databases(&main_path) {
				warn!("orphan database recovery failed: {}", e);
			}
		});
	}

	/// Recover orphaned working databases
	fn recover_orphan_databases(main_path: &Path) -> Result<()> {
		let orphan_dbs = Self::find_orphan_databases(main_path)?;

		if orphan_dbs.is_empty() {
			return Ok(());
		}

		info!("found {} orphaned database(s) to recover", orphan_dbs.len());

		for orphan_path in orphan_dbs {
			// Try to open exclusively to check if it's in use
			match Self::try_open_working_exclusive(&orphan_path) {
				Ok(_db) => {
					// Successfully opened exclusively, so it's not in use
					drop(_db);

					// Merge into main database with retries
					for attempt in 1..=MAX_EXIT_RETRIES {
						match Database::create(main_path) {
							Ok(_main_db) => {
								drop(_main_db);
								match Self::merge_working_into_main(&orphan_path, main_path) {
									Ok(()) => {
										// Delete the orphan database file
										if let Err(e) = fs::remove_file(&orphan_path) {
											warn!(
												"failed to delete orphan database {:?}: {}",
												orphan_path, e
											);
										} else {
											info!("recovered orphan database: {:?}", orphan_path);
										}
										break;
									}
									Err(e) => {
										warn!(
											"failed to merge orphan database {:?}: {}",
											orphan_path, e
										);
										break;
									}
								}
							}
							Err(e) => {
								if attempt < MAX_EXIT_RETRIES {
									let delay = Self::random_retry_delay();
									debug!(
										"failed to open main database for orphaned database recovery (attempt {}/{}), retrying in {:?}: {}",
										attempt, MAX_EXIT_RETRIES, delay, e
									);
									thread::sleep(delay);
								} else {
									warn!(
										"failed to open main database to recover orphaned database {:?} after {} attempts",
										orphan_path, MAX_EXIT_RETRIES
									);
									break;
								}
							}
						}
					}
				}
				Err(_) => {
					// Could not open exclusively, likely still in use
					debug!(
						"working database {:?} is still in use, skipping",
						orphan_path
					);
				}
			}
		}

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_working_database_generation() {
		let temp_dir = tempfile::tempdir().unwrap();
		let main_path = temp_dir.path().join("audit-main.redb");

		let (working_path, uuid) = WorkingDatabase::generate_path(&main_path);

		// Check that the working database path is in the same directory
		assert_eq!(working_path.parent().unwrap(), temp_dir.path());

		// Check that the filename matches the expected pattern
		let filename = working_path.file_name().unwrap().to_str().unwrap();
		assert!(filename.starts_with(WORKING_PREFIX));
		assert!(filename.ends_with(DB_SUFFIX));
		assert!(filename.contains(&uuid.to_string()));

		// Check orphaned path generation
		let working_info = WorkingDatabase::new(main_path.clone(), working_path.clone(), uuid);
		let orphaned_path = working_info.orphaned_path();
		let orphaned_filename = orphaned_path.file_name().unwrap().to_str().unwrap();
		assert!(orphaned_filename.starts_with(ORPHANED_PREFIX));
		assert!(orphaned_filename.ends_with(DB_SUFFIX));
		assert!(orphaned_filename.contains(&uuid.to_string()));
	}

	#[test]
	fn test_find_orphan_databases() {
		let temp_dir = tempfile::tempdir().unwrap();
		let main_path = temp_dir.path().join("audit-main.redb");

		// Create an explicitly orphaned database
		let orphaned1 = temp_dir.path().join("audit-orphaned-test1.redb");
		std::fs::write(&orphaned1, b"test").unwrap();

		// Create a working database with old mtime (crash recovery case)
		let working1 = temp_dir.path().join("audit-working-test2.redb");
		std::fs::write(&working1, b"test").unwrap();

		// Set mtime to more than 1 minute ago
		#[cfg(unix)]
		{
			let old_time = SystemTime::now() - Duration::from_secs(120);
			filetime::set_file_mtime(&working1, filetime::FileTime::from_system_time(old_time))
				.unwrap();
		}

		#[cfg(not(unix))]
		{
			// On non-Unix systems, just sleep to make files old
			std::thread::sleep(Duration::from_secs(2));
		}

		// Create a recent working database that should not be found
		let working2 = temp_dir.path().join("audit-working-test3.redb");
		std::fs::write(&working2, b"test").unwrap();

		// Create a non-working database file
		let other = temp_dir.path().join("other.redb");
		std::fs::write(&other, b"test").unwrap();

		#[cfg(unix)]
		{
			let orphan_dbs = WorkingDatabase::find_orphan_databases(&main_path).unwrap();
			assert_eq!(orphan_dbs.len(), 2);
			// Should find the explicitly orphaned one
			assert!(orphan_dbs.contains(&orphaned1));
			// Should find the old working one (crash recovery)
			assert!(orphan_dbs.contains(&working1));
			// Should NOT find the recent working one
			assert!(!orphan_dbs.contains(&working2));
			// Should NOT find the non-audit file
			assert!(!orphan_dbs.contains(&other));
		}
	}

	#[test]
	fn test_multi_process_basic() {
		let temp_dir = tempfile::tempdir().unwrap();

		// Create first audit instance using open_empty to avoid psql history import
		let mut audit1 = crate::audit::Audit::open_empty(temp_dir.path()).unwrap();

		// Add some entries
		audit1.add_entry("SELECT 1;".to_string()).unwrap();
		audit1.add_entry("SELECT 2;".to_string()).unwrap();

		// Verify entries are in the working database
		let entries = audit1.list().unwrap();
		assert_eq!(entries.len(), 2);

		// Cleanup
		drop(audit1);
	}
}

/// Sync new entries from working database to main database
pub fn sync_to_main(working_db: &Database, working_info: &WorkingDatabase) -> Result<()> {
	debug!("syncing working database to main database");

	let last_synced = working_info.last_synced_timestamp.load(Ordering::Relaxed);

	// Read entries from working database that are newer than last_synced
	let working_read_txn = working_db.begin_read().into_diagnostic()?;
	let working_history = match working_read_txn.open_table(super::HISTORY_TABLE) {
		Ok(table) => table,
		Err(_) => {
			debug!("working database has no history table");
			return Ok(());
		}
	};

	let mut new_entries = Vec::new();
	let mut max_timestamp = last_synced;

	for item in working_history.iter().into_diagnostic()? {
		let (timestamp, json) = item.into_diagnostic()?;
		let ts = timestamp.value();
		if ts > last_synced {
			new_entries.push((ts, json.value().to_string()));
			max_timestamp = max_timestamp.max(ts);
		}
	}
	drop(working_history);
	drop(working_read_txn);

	if new_entries.is_empty() {
		debug!("no new entries to sync");
		return Ok(());
	}

	// Open main database and write entries
	let main_db = working_info.open_main_readwrite(MAX_EXIT_RETRIES, true)?;
	let main_write_txn = main_db.begin_write().into_diagnostic()?;
	{
		let mut main_history = main_write_txn
			.open_table(super::HISTORY_TABLE)
			.into_diagnostic()?;

		for (timestamp, json) in &new_entries {
			main_history
				.insert(*timestamp, json.as_str())
				.into_diagnostic()?;
		}
	}
	main_write_txn.commit().into_diagnostic()?;
	drop(main_db);

	// Update last synced timestamp
	working_info
		.last_synced_timestamp
		.store(max_timestamp, Ordering::Relaxed);

	debug!("synced {} new entries to main database", new_entries.len());
	Ok(())
}

/// Spawn background sync task
pub fn spawn_sync_task(working_db: Arc<Database>, working_info: Arc<WorkingDatabase>) {
	thread::spawn(move || {
		loop {
			// Wait for sync interval or shutdown
			let start = std::time::Instant::now();
			while start.elapsed() < SYNC_INTERVAL {
				if working_info.shutdown.load(Ordering::Relaxed) {
					return;
				}
				thread::sleep(Duration::from_millis(100));
			}

			if working_info.shutdown.load(Ordering::Relaxed) {
				return;
			}

			// Perform sync
			if let Err(e) = sync_to_main(&working_db, &working_info) {
				warn!("periodic sync failed: {}", e);
			} else {
				debug!("periodic sync completed");
			}
		}
	});
}
