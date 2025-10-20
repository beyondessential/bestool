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
pub struct History {
	db: Database,
	/// Sorted list of timestamps for indexed access
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
}

impl History {
	/// Open or create a history database at the given path
	pub fn open(path: impl AsRef<Path>) -> Result<Self> {
		let db = Database::create(path).into_diagnostic()?;

		// Load all timestamps for indexed access
		let timestamps = Self::load_timestamps(&db)?;

		Ok(Self {
			db,
			timestamps,
			max_len: 10000,
			ignore_dups: true,
			ignore_space: false,
			db_user: String::new(),
			sys_user: String::new(),
			writemode: false,
		})
	}

	/// Set the context for new history entries
	pub fn set_context(&mut self, db_user: String, sys_user: String, writemode: bool) {
		self.db_user = db_user;
		self.sys_user = sys_user;
		self.writemode = writemode;
	}

	/// Load all timestamps from the database
	fn load_timestamps(db: &Database) -> Result<Vec<u64>> {
		let read_txn = db.begin_read().into_diagnostic()?;

		// Try to open the table, but if it doesn't exist yet, return empty vec
		let table = match read_txn.open_table(HISTORY_TABLE) {
			Ok(table) => table,
			Err(_) => return Ok(Vec::new()), // Table doesn't exist yet
		};

		let mut timestamps = Vec::new();
		for item in table.iter().into_diagnostic()? {
			let (timestamp, _) = item.into_diagnostic()?;
			timestamps.push(timestamp.value());
		}

		Ok(timestamps)
	}

	/// Get entry by timestamp
	fn get_entry(&self, timestamp: u64) -> Result<HistoryEntry> {
		let read_txn = self.db.begin_read().into_diagnostic()?;
		let table = read_txn.open_table(HISTORY_TABLE).into_diagnostic()?;

		let json = table
			.get(timestamp)
			.into_diagnostic()?
			.ok_or_else(|| miette::miette!("Entry not found"))?;

		serde_json::from_str(json.value()).into_diagnostic()
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
	) -> Result<()> {
		let tailscale = get_tailscale_peers().ok().unwrap_or_default();

		let entry = HistoryEntry {
			query,
			db_user,
			sys_user,
			writemode,
			tailscale,
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

		// Update timestamps index
		self.timestamps.push(timestamp);

		// Enforce max length
		if self.timestamps.len() > self.max_len {
			let to_remove = self.timestamps.len() - self.max_len;
			let old_timestamps: Vec<u64> = self.timestamps.drain(..to_remove).collect();

			// Remove from database
			let write_txn = self.db.begin_write().into_diagnostic()?;
			{
				let mut table = write_txn.open_table(HISTORY_TABLE).into_diagnostic()?;
				for ts in old_timestamps {
					table.remove(ts).into_diagnostic()?;
				}
			}
			write_txn.commit().into_diagnostic()?;
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

		Ok(Some(SearchResult {
			entry: Cow::Owned(entry.query),
			idx: index,
			pos: 0,
		}))
	}

	fn add(&mut self, line: &str) -> rustyline::Result<bool> {
		self.add_owned(line.to_string())
	}

	fn add_owned(&mut self, line: String) -> rustyline::Result<bool> {
		// Check ignore rules
		if line.trim().is_empty() {
			return Ok(false);
		}

		if self.ignore_space && line.starts_with(' ') {
			return Ok(false);
		}

		if self.ignore_dups && !self.timestamps.is_empty() {
			// Check if the last entry is a duplicate
			if let Ok(last_entry) = self.get_entry(self.timestamps[self.timestamps.len() - 1]) {
				if last_entry.query == line {
					return Ok(false);
				}
			}
		}

		// Add to database
		self.add_entry(
			line,
			self.db_user.clone(),
			self.sys_user.clone(),
			self.writemode,
		)
		.map_err(|e| {
			rustyline::error::ReadlineError::Io(std::io::Error::new(
				std::io::ErrorKind::Other,
				e.to_string(),
			))
		})?;

		Ok(true)
	}

	fn len(&self) -> usize {
		self.timestamps.len()
	}

	fn is_empty(&self) -> bool {
		self.timestamps.is_empty()
	}

	fn set_max_len(&mut self, len: usize) -> rustyline::Result<()> {
		self.max_len = len;

		// Trim history if needed
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
		self.ignore_dups = yes;
		Ok(())
	}

	fn ignore_space(&mut self, yes: bool) {
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
		self.clear_all().map_err(|e| {
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

	// Check if tailscale is installed
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

	// Get the User map for looking up user info by UserID
	let user_map = json.get("User").and_then(|u| u.as_object());

	if let Some(peer_map) = json.get("Peer").and_then(|p| p.as_object()) {
		for (_key, peer) in peer_map {
			// Check if peer is active
			let active = peer
				.get("Active")
				.and_then(|a| a.as_bool())
				.unwrap_or(false);

			if !active {
				continue;
			}

			// Check if peer has no tags (or Tags is null)
			let has_tags = peer
				.get("Tags")
				.and_then(|t| t.as_array())
				.map(|arr| !arr.is_empty())
				.unwrap_or(false);

			if has_tags {
				continue;
			}

			// Extract hostname
			let device = peer
				.get("HostName")
				.and_then(|h| h.as_str())
				.map(|s| s.to_string());

			// Get UserID and look up the user info
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
			)
			.unwrap();
		history
			.add_entry(
				"SELECT 2;".to_string(),
				"dbuser".to_string(),
				"testuser".to_string(),
				false,
			)
			.unwrap();
		history
			.add_entry(
				"INSERT INTO foo;".to_string(),
				"dbuser".to_string(),
				"testuser".to_string(),
				true,
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
		history.set_context("dbuser".to_string(), "sysuser".to_string(), false);

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
