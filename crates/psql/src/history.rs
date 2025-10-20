//! Query history storage using redb.
//!
//! History entries are stored with timestamp keys and JSON-serialized values
//! containing the query, user, and write mode information.

use miette::{IntoDiagnostic, Result};
use redb::{Database, ReadableTable, TableDefinition};
use rustyline::history::{History as HistoryTrait, SearchDirection, SearchResult};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use tracing::{debug, trace, warn};

const HISTORY_TABLE: TableDefinition<u64, &str> = TableDefinition::new("history");

/// A single history entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
	/// The SQL query that was executed
	pub query: String,
	/// The database user
	pub db_user: String,
	/// The OS-level user (e.g. $USER on Unix)
	pub sys_user: String,
	/// Whether write mode was enabled
	pub writemode: bool,
	/// Tailscale peer information (if tailscale is installed and has active peers)
	#[serde(skip_serializing_if = "Vec::is_empty", default)]
	pub tailscale: Vec<TailscalePeer>,
	/// OTS (Over The Shoulder) value for write mode sessions
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub ots: Option<String>,
}

/// Information about a Tailscale peer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TailscalePeer {
	/// Device hostname
	pub device: String,
	/// User login name
	pub user: String,
}

/// History manager using redb for persistent storage
///
/// This struct is safe for use with concurrent writers. Multiple psql processes
/// can write to the same database simultaneously. The in-memory `timestamps` cache
/// may become stale if other processes add entries, but operations remain safe:
/// - Database operations use redb's MVCC for consistency
/// - Missing entries are handled gracefully
/// - Timestamps can be refreshed with `reload_timestamps()`
pub struct History {
	db: Database,
	/// Sorted list of timestamps for indexed access (may be stale with concurrent writers)
	timestamps: Vec<u64>,
	/// Maximum history length
	max_len: usize,
	/// Ignore consecutive duplicates
	ignore_dups: bool,
	/// Ignore lines starting with space
	ignore_space: bool,
	/// Database user for new entries
	db_user: String,
	/// System user for new entries
	sys_user: String,
	/// Write mode for new entries
	writemode: bool,
	/// OTS value for new entries
	ots: Option<String>,
}

impl History {
	/// Open or create a history database at the given path
	pub fn open(path: impl AsRef<Path>) -> Result<Self> {
		let path = path.as_ref();
		let db = Database::create(path).into_diagnostic()?;

		let mut timestamps = Self::load_timestamps(&db)?;

		if let Ok(metadata) = std::fs::metadata(path) {
			const MAX_SIZE: u64 = 100 * 1024 * 1024; // 100MB
			const TARGET_SIZE: u64 = 90 * 1024 * 1024; // 90MB
			const CULL_BATCH: usize = 100; // Remove 100 entries at a time

			if metadata.len() > MAX_SIZE {
				let size_mb = metadata.len() / (1024 * 1024);
				warn!(size_mb, "history database exceeds 100MB, culling to 90MB");

				// Remove oldest entries in batches until we reach target size
				while timestamps.len() > 0 {
					if let Ok(metadata) = std::fs::metadata(path) {
						if metadata.len() <= TARGET_SIZE {
							break;
						}
					}

					let to_remove = CULL_BATCH.min(timestamps.len());
					let old_timestamps: Vec<u64> = timestamps.drain(..to_remove).collect();

					let write_txn = db.begin_write().into_diagnostic()?;
					{
						let mut table = write_txn.open_table(HISTORY_TABLE).into_diagnostic()?;
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
					"culled history database"
				);
			}
		}

		Ok(Self {
			db,
			timestamps,
			max_len: 10000,
			ignore_dups: true,
			ignore_space: false,
			db_user: String::new(),
			sys_user: String::new(),
			writemode: false,
			ots: None,
		})
	}

	/// Set the context for new history entries
	pub fn set_context(
		&mut self,
		db_user: String,
		sys_user: String,
		writemode: bool,
		ots: Option<String>,
	) {
		debug!(
			?db_user,
			?sys_user,
			writemode,
			?ots,
			"setting history context"
		);
		self.db_user = db_user;
		self.sys_user = sys_user;
		self.writemode = writemode;
		self.ots = ots;
	}

	/// Load all timestamps from the database
	fn load_timestamps(db: &Database) -> Result<Vec<u64>> {
		let read_txn = db.begin_read().into_diagnostic()?;

		let table = match read_txn.open_table(HISTORY_TABLE) {
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

	/// Reload timestamps from the database
	///
	/// This is useful when multiple processes are writing to the same database.
	/// Call this to see entries added by other processes.
	pub fn reload_timestamps(&mut self) -> Result<()> {
		let old_len = self.timestamps.len();
		self.timestamps = Self::load_timestamps(&self.db)?;
		let new_len = self.timestamps.len();
		if new_len != old_len {
			debug!(old_len, new_len, "reloaded history timestamps");
		} else {
			trace!("reloaded history timestamps (no change)");
		}
		Ok(())
	}

	/// Get entry by timestamp
	///
	/// Returns None if the entry doesn't exist (may have been deleted by another process)
	fn get_entry(&self, timestamp: u64) -> Result<Option<HistoryEntry>> {
		let read_txn = self.db.begin_read().into_diagnostic()?;

		let table = match read_txn.open_table(HISTORY_TABLE) {
			Ok(table) => table,
			Err(_) => return Ok(None),
		};

		let json = match table.get(timestamp).into_diagnostic()? {
			Some(json) => json,
			None => return Ok(None),
		};

		let entry = serde_json::from_str(json.value()).into_diagnostic()?;
		Ok(Some(entry))
	}

	/// Compact the database to reclaim space from deleted entries
	pub fn compact(&mut self) -> Result<()> {
		self.db.compact().into_diagnostic()?;
		Ok(())
	}

	/// Get the default history database path
	pub fn default_path() -> Result<PathBuf> {
		let cache_dir = if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
			PathBuf::from(dir)
		} else if let Some(home) = std::env::var_os("HOME") {
			PathBuf::from(home).join(".cache")
		} else if let Some(localappdata) = std::env::var_os("LOCALAPPDATA") {
			// Windows
			PathBuf::from(localappdata)
		} else {
			return Err(miette::miette!("Could not determine cache directory"));
		};

		let history_dir = cache_dir.join("bestool-psql");
		std::fs::create_dir_all(&history_dir).into_diagnostic()?;
		Ok(history_dir.join("history.redb"))
	}

	/// Add a new entry to the history (legacy method for compatibility)
	pub fn add_entry(
		&mut self,
		query: String,
		db_user: String,
		sys_user: String,
		writemode: bool,
		ots: Option<String>,
	) -> Result<()> {
		trace!("adding history entry");
		let tailscale = get_tailscale_peers().ok().unwrap_or_default();

		let entry = HistoryEntry {
			query,
			db_user,
			sys_user,
			writemode,
			tailscale,
			ots,
		};

		let json = serde_json::to_string(&entry).into_diagnostic()?;
		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.into_diagnostic()?
			.as_micros() as u64;

		let write_txn = self.db.begin_write().into_diagnostic()?;
		{
			let mut table = write_txn.open_table(HISTORY_TABLE).into_diagnostic()?;
			table.insert(timestamp, json.as_str()).into_diagnostic()?;
		}
		write_txn.commit().into_diagnostic()?;

		self.timestamps.push(timestamp);

		// Enforce max length (note: other processes may have added entries,
		// so the actual database size may exceed max_len temporarily)
		if self.timestamps.len() > self.max_len {
			let to_remove = self.timestamps.len() - self.max_len;
			let old_timestamps: Vec<u64> = self.timestamps.drain(..to_remove).collect();

			// Remove from database (ignore errors if already deleted by another process)
			if let Ok(write_txn) = self.db.begin_write() {
				{
					if let Ok(mut table) = write_txn.open_table(HISTORY_TABLE) {
						for ts in old_timestamps {
							let _ = table.remove(ts);
						}
					}
				}
				let _ = write_txn.commit();
			}
		}

		Ok(())
	}

	/// Get all history entries in chronological order (oldest first)
	pub fn list(&self) -> Result<Vec<(u64, HistoryEntry)>> {
		let read_txn = self.db.begin_read().into_diagnostic()?;
		let table = read_txn.open_table(HISTORY_TABLE).into_diagnostic()?;

		let mut entries = Vec::new();
		for item in table.iter().into_diagnostic()? {
			let (timestamp, json) = item.into_diagnostic()?;
			let entry: HistoryEntry = serde_json::from_str(json.value()).into_diagnostic()?;
			entries.push((timestamp.value(), entry));
		}

		Ok(entries)
	}

	/// Get the most recent N history entries (newest first)
	pub fn recent(&self, limit: usize) -> Result<Vec<(u64, HistoryEntry)>> {
		let mut all = self.list()?;
		all.reverse();
		all.truncate(limit);
		Ok(all)
	}

	/// Get all queries (deduplicated, most recent first) for rustyline history
	pub fn queries_for_rustyline(&self) -> Result<Vec<String>> {
		let entries = self.list()?;
		let mut queries = Vec::new();
		let mut seen = std::collections::HashSet::new();

		// Iterate in reverse to get most recent first
		for (_, entry) in entries.into_iter().rev() {
			if seen.insert(entry.query.clone()) {
				queries.push(entry.query);
			}
		}

		Ok(queries)
	}

	/// Clear all history
	pub fn clear_all(&mut self) -> Result<()> {
		let write_txn = self.db.begin_write().into_diagnostic()?;
		{
			let mut table = write_txn.open_table(HISTORY_TABLE).into_diagnostic()?;
			// Collect all keys first to avoid iterator invalidation
			let keys: Vec<u64> = table
				.iter()
				.into_diagnostic()?
				.filter_map(|item| item.ok())
				.map(|(k, _)| k.value())
				.collect();

			for key in keys {
				table.remove(key).into_diagnostic()?;
			}
		}
		write_txn.commit().into_diagnostic()?;
		self.timestamps.clear();
		Ok(())
	}
}

/// Implementation of rustyline's History trait for database-backed history
impl HistoryTrait for History {
	fn get(
		&self,
		index: usize,
		_dir: SearchDirection,
	) -> rustyline::Result<Option<SearchResult<'_>>> {
		if index >= self.timestamps.len() {
			return Ok(None);
		}

		let timestamp = self.timestamps[index];
		let entry = self.get_entry(timestamp).map_err(|e| {
			rustyline::error::ReadlineError::Io(std::io::Error::new(
				std::io::ErrorKind::Other,
				e.to_string(),
			))
		})?;

		// Entry may have been deleted by another process
		let entry = match entry {
			Some(e) => e,
			None => return Ok(None),
		};

		Ok(Some(SearchResult {
			entry: Cow::Owned(entry.query),
			idx: index,
			pos: 0,
		}))
	}

	fn add(&mut self, line: &str) -> rustyline::Result<bool> {
		trace!("History::add called");
		self.add_owned(line.to_string())
	}

	fn add_owned(&mut self, line: String) -> rustyline::Result<bool> {
		if line.trim().is_empty() {
			trace!("ignoring empty line");
			return Ok(false);
		}

		if self.ignore_space && line.starts_with(' ') {
			trace!("ignoring line starting with space");
			return Ok(false);
		}

		if self.ignore_dups && !self.timestamps.is_empty() {
			if let Ok(Some(last_entry)) = self.get_entry(self.timestamps[self.timestamps.len() - 1])
			{
				if last_entry.query == line {
					trace!("ignoring duplicate entry");
					return Ok(false);
				}
			}
		}

		self.add_entry(
			line,
			self.db_user.clone(),
			self.sys_user.clone(),
			self.writemode,
			self.ots.clone(),
		)
		.map_err(|e| {
			warn!("failed to add history entry: {}", e);
			rustyline::error::ReadlineError::Io(std::io::Error::new(
				std::io::ErrorKind::Other,
				e.to_string(),
			))
		})?;

		debug!("added history entry");
		Ok(true)
	}

	fn len(&self) -> usize {
		self.timestamps.len()
	}

	fn is_empty(&self) -> bool {
		self.timestamps.is_empty()
	}

	fn set_max_len(&mut self, len: usize) -> rustyline::Result<()> {
		debug!(len, "setting max history length");
		self.max_len = len;

		if self.timestamps.len() > len {
			let to_remove = self.timestamps.len() - len;
			let old_timestamps: Vec<u64> = self.timestamps.drain(..to_remove).collect();

			// Remove from database
			let write_txn = self.db.begin_write().map_err(|e| {
				rustyline::error::ReadlineError::Io(std::io::Error::new(
					std::io::ErrorKind::Other,
					e.to_string(),
				))
			})?;
			{
				let mut table = write_txn.open_table(HISTORY_TABLE).map_err(|e| {
					rustyline::error::ReadlineError::Io(std::io::Error::new(
						std::io::ErrorKind::Other,
						e.to_string(),
					))
				})?;
				for ts in old_timestamps {
					table.remove(ts).map_err(|e| {
						rustyline::error::ReadlineError::Io(std::io::Error::new(
							std::io::ErrorKind::Other,
							e.to_string(),
						))
					})?;
				}
			}
			write_txn.commit().map_err(|e| {
				rustyline::error::ReadlineError::Io(std::io::Error::new(
					std::io::ErrorKind::Other,
					e.to_string(),
				))
			})?;
		}

		Ok(())
	}

	fn ignore_dups(&mut self, yes: bool) -> rustyline::Result<()> {
		debug!(yes, "setting ignore_dups");
		self.ignore_dups = yes;
		Ok(())
	}

	fn ignore_space(&mut self, yes: bool) {
		debug!(yes, "setting ignore_space");
		self.ignore_space = yes;
	}

	fn save(&mut self, _path: &Path) -> rustyline::Result<()> {
		// No-op: already persisted to database
		Ok(())
	}

	fn append(&mut self, _path: &Path) -> rustyline::Result<()> {
		// No-op: already persisted to database
		Ok(())
	}

	fn load(&mut self, _path: &Path) -> rustyline::Result<()> {
		// No-op: loaded from database
		Ok(())
	}

	fn clear(&mut self) -> rustyline::Result<()> {
		debug!("clearing history");
		self.clear_all().map_err(|e| {
			warn!("failed to clear history: {}", e);
			rustyline::error::ReadlineError::Io(std::io::Error::new(
				std::io::ErrorKind::Other,
				e.to_string(),
			))
		})
	}

	fn search(
		&self,
		term: &str,
		start: usize,
		dir: SearchDirection,
	) -> rustyline::Result<Option<SearchResult<'_>>> {
		let range: Box<dyn Iterator<Item = usize>> = match dir {
			SearchDirection::Forward => {
				if start >= self.timestamps.len() {
					return Ok(None);
				}
				Box::new(start..self.timestamps.len())
			}
			SearchDirection::Reverse => {
				if start >= self.timestamps.len() {
					return Ok(None);
				}
				Box::new((0..=start).rev())
			}
		};

		for idx in range {
			let timestamp = self.timestamps[idx];
			let entry = self.get_entry(timestamp).map_err(|e| {
				rustyline::error::ReadlineError::Io(std::io::Error::new(
					std::io::ErrorKind::Other,
					e.to_string(),
				))
			})?;

			let entry = match entry {
				Some(e) => e,
				None => continue,
			};

			if let Some(pos) = entry.query.find(term) {
				return Ok(Some(SearchResult {
					entry: Cow::Owned(entry.query),
					idx,
					pos,
				}));
			}
		}

		Ok(None)
	}

	fn starts_with(
		&self,
		term: &str,
		start: usize,
		dir: SearchDirection,
	) -> rustyline::Result<Option<SearchResult<'_>>> {
		let range: Box<dyn Iterator<Item = usize>> = match dir {
			SearchDirection::Forward => {
				if start >= self.timestamps.len() {
					return Ok(None);
				}
				Box::new(start..self.timestamps.len())
			}
			SearchDirection::Reverse => {
				if start >= self.timestamps.len() {
					return Ok(None);
				}
				Box::new((0..=start).rev())
			}
		};

		for idx in range {
			let timestamp = self.timestamps[idx];
			let entry = self.get_entry(timestamp).map_err(|e| {
				rustyline::error::ReadlineError::Io(std::io::Error::new(
					std::io::ErrorKind::Other,
					e.to_string(),
				))
			})?;

			let entry = match entry {
				Some(e) => e,
				None => continue,
			};

			if entry.query.starts_with(term) {
				return Ok(Some(SearchResult {
					entry: Cow::Owned(entry.query),
					idx,
					pos: 0,
				}));
			}
		}

		Ok(None)
	}
}

/// Get active Tailscale peers without tags
fn get_tailscale_peers() -> Result<Vec<TailscalePeer>> {
	use std::process::Command;

	let output = Command::new("tailscale")
		.arg("status")
		.arg("--json")
		.output()
		.into_diagnostic()?;

	if !output.status.success() {
		return Err(miette::miette!("tailscale command failed"));
	}

	let json: serde_json::Value = serde_json::from_slice(&output.stdout).into_diagnostic()?;

	let mut peers = Vec::new();

	let user_map = json.get("User").and_then(|u| u.as_object());

	if let Some(peer_map) = json.get("Peer").and_then(|p| p.as_object()) {
		for (_key, peer) in peer_map {
			let active = peer
				.get("Active")
				.and_then(|a| a.as_bool())
				.unwrap_or(false);

			if !active {
				continue;
			}

			let has_tags = peer
				.get("Tags")
				.and_then(|t| t.as_array())
				.map(|arr| !arr.is_empty())
				.unwrap_or(false);

			if has_tags {
				continue;
			}

			let device = peer
				.get("HostName")
				.and_then(|h| h.as_str())
				.map(|s| s.to_string());

			let user_id = peer.get("UserID").and_then(|id| id.as_u64());

			let user = if let (Some(user_map), Some(user_id)) = (user_map, user_id) {
				user_map
					.get(&user_id.to_string())
					.and_then(|u| u.get("LoginName"))
					.and_then(|l| l.as_str())
					.map(|s| s.to_string())
			} else {
				None
			};

			if let (Some(device), Some(user)) = (device, user) {
				peers.push(TailscalePeer { device, user });
			}
		}
	}

	if peers.is_empty() {
		Err(miette::miette!("no active tailscale peers found"))
	} else {
		Ok(peers)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_history_roundtrip() {
		let temp_dir = tempfile::tempdir().unwrap();
		let db_path = temp_dir.path().join("test.redb");

		let mut history = History::open(db_path).unwrap();

		// Add some entries
		history
			.add_entry(
				"SELECT 1;".to_string(),
				"dbuser".to_string(),
				"testuser".to_string(),
				false,
				None,
			)
			.unwrap();
		history
			.add_entry(
				"SELECT 2;".to_string(),
				"dbuser".to_string(),
				"testuser".to_string(),
				false,
				None,
			)
			.unwrap();
		history
			.add_entry(
				"INSERT INTO foo;".to_string(),
				"dbuser".to_string(),
				"testuser".to_string(),
				true,
				Some("John Doe".to_string()),
			)
			.unwrap();

		// List all entries
		let entries = history.list().unwrap();
		assert_eq!(entries.len(), 3);
		assert_eq!(entries[0].1.query, "SELECT 1;");
		assert_eq!(entries[1].1.query, "SELECT 2;");
		assert_eq!(entries[2].1.query, "INSERT INTO foo;");
		assert_eq!(entries[2].1.writemode, true);
		assert_eq!(entries[2].1.db_user, "dbuser");
		assert_eq!(entries[2].1.sys_user, "testuser");
		assert_eq!(entries[2].1.ots, Some("John Doe".to_string()));

		// Get recent entries
		let recent = history.recent(2).unwrap();
		assert_eq!(recent.len(), 2);
		assert_eq!(recent[0].1.query, "INSERT INTO foo;");
		assert_eq!(recent[1].1.query, "SELECT 2;");

		// Get queries for rustyline
		let queries = history.queries_for_rustyline().unwrap();
		assert_eq!(queries.len(), 3);
		assert_eq!(queries[0], "INSERT INTO foo;");
		assert_eq!(queries[1], "SELECT 2;");
		assert_eq!(queries[2], "SELECT 1;");
	}

	#[test]
	fn test_rustyline_history_trait() {
		use rustyline::history::History as HistoryTrait;

		let temp_dir = tempfile::tempdir().unwrap();
		let db_path = temp_dir.path().join("test.redb");

		let mut history = History::open(db_path).unwrap();
		history.set_context("dbuser".to_string(), "sysuser".to_string(), false, None);

		// Test add
		assert!(history.add("SELECT 1;").unwrap());
		assert!(history.add("SELECT 2;").unwrap());
		assert!(history.add("SELECT 3;").unwrap());

		// Test len
		assert_eq!(history.len(), 3);
		assert!(!history.is_empty());

		// Test get
		let result = history.get(0, SearchDirection::Forward).unwrap().unwrap();
		assert_eq!(result.entry, "SELECT 1;");
		assert_eq!(result.idx, 0);

		let result = history.get(2, SearchDirection::Forward).unwrap().unwrap();
		assert_eq!(result.entry, "SELECT 3;");
		assert_eq!(result.idx, 2);

		// Test ignore_dups (default is true)
		assert!(!history.add("SELECT 3;").unwrap()); // Should NOT add duplicate (default behavior)
		assert_eq!(history.len(), 3);

		// Disable ignore_dups
		history.ignore_dups(false).unwrap();
		assert!(history.add("SELECT 3;").unwrap()); // Should add duplicate now
		assert_eq!(history.len(), 4);

		// Test search
		let result = history
			.search("SELECT 2", 0, SearchDirection::Forward)
			.unwrap()
			.unwrap();
		assert_eq!(result.entry, "SELECT 2;");
		assert_eq!(result.pos, 0);

		// Test starts_with
		let result = history
			.starts_with("SELECT 3", 0, SearchDirection::Forward)
			.unwrap()
			.unwrap();
		assert_eq!(result.entry, "SELECT 3;");

		// Test max_len
		history.set_max_len(2).unwrap();
		assert_eq!(history.len(), 2);
		// Should keep the most recent entries
		let result = history.get(0, SearchDirection::Forward).unwrap().unwrap();
		assert_eq!(result.entry, "SELECT 3;");

		// Test clear
		history.clear().unwrap();
		assert_eq!(history.len(), 0);
		assert!(history.is_empty());
	}
}
