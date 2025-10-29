//! Query audit storage using redb.
//!
//! Audit entries are stored with timestamp keys and JSON-serialized values
//! containing the query, user, and write mode information.

use miette::{IntoDiagnostic, Result};
use redb::backends::InMemoryBackend;
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use rustyline::history::{History as RustylineHistory, SearchDirection, SearchResult};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::mem::replace;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, instrument, trace, warn};

use crate::repl::ReplState;

pub const HISTORY_TABLE: TableDefinition<u64, &str> = TableDefinition::new("history");

/// A single audit entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
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

/// Audit manager using redb for persistent storage
///
/// This struct is safe for use with concurrent writers. Multiple psql processes
/// can write to the same database simultaneously. The in-memory `timestamps` cache
/// may become stale if other processes add entries, but operations remain safe:
/// - Database operations use redb's MVCC for consistency
/// - Missing entries are handled gracefully
/// - Timestamps can be refreshed with `reload_timestamps()`
#[derive(Debug)]
pub struct Audit {
	pub(crate) db: Arc<Database>,
	/// Sorted list of timestamps for indexed access (may be stale with concurrent writers)
	pub(crate) timestamps: Vec<u64>,
	/// State to record as context for new entries
	pub repl_state: ReplState,
}

impl Audit {
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
				debug!("could not import psql history: {}", e);
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

	/// Set the context for new history entries from REPL state
	#[instrument(level = "debug")]
	pub fn set_repl_state(&mut self, repl_state: &ReplState) {
		self.repl_state = repl_state.clone();
	}

	/// Get entry by timestamp
	///
	/// Returns None if the entry doesn't exist (may have been deleted by another process)
	fn get_entry(&self, timestamp: u64) -> Result<Option<AuditEntry>> {
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

	/// Add a new entry to the audit
	pub fn add_entry(&mut self, query: String) -> Result<()> {
		trace!("adding audit entry");
		let tailscale = get_tailscale_peers().ok().unwrap_or_default();

		let entry = AuditEntry {
			query,
			db_user: self.repl_state.db_user.clone(),
			sys_user: self.repl_state.sys_user.clone(),
			writemode: self.repl_state.write_mode,
			tailscale,
			ots: self.repl_state.ots.clone(),
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

		Ok(())
	}

	/// Get all audit entries in chronological order (oldest first)
	pub fn list(&self) -> Result<Vec<(u64, AuditEntry)>> {
		let read_txn = self.db.begin_read().into_diagnostic()?;
		let table = read_txn.open_table(HISTORY_TABLE).into_diagnostic()?;

		let mut entries = Vec::new();
		for item in table.iter().into_diagnostic()? {
			let (timestamp, json) = item.into_diagnostic()?;
			let entry: AuditEntry = serde_json::from_str(json.value()).into_diagnostic()?;
			entries.push((timestamp.value(), entry));
		}

		Ok(entries)
	}
}

/// Load all timestamps from the database
#[instrument(level = "trace", skip(db))]
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
		debug!("no psql history file found at {:?}", psql_history_path);
		return Ok(());
	}

	info!("importing psql history from {:?}", psql_history_path);

	let content = std::fs::read_to_string(&psql_history_path).into_diagnostic()?;
	let lines: Vec<&str> = content.lines().collect();

	if lines.is_empty() {
		return Ok(());
	}

	let write_txn = db.begin_write().into_diagnostic()?;
	{
		let mut table = write_txn.open_table(HISTORY_TABLE).into_diagnostic()?;
		let mut timestamp = 0u64;

		for line in lines {
			let line = line.trim();
			if line.is_empty() {
				continue;
			}

			// Create entry with default values
			let entry = AuditEntry {
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

/// Implementation of rustyline's History trait for database-backed history
impl RustylineHistory for Audit {
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
			rustyline::error::ReadlineError::Io(std::io::Error::other(e.to_string()))
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

	fn add(&mut self, _line: &str) -> rustyline::Result<bool> {
		trace!("Audit::add called and ignored");
		Ok(true)
	}

	fn add_owned(&mut self, _line: String) -> rustyline::Result<bool> {
		trace!("Audit::add_owned called and ignored");
		Ok(true)
	}

	fn len(&self) -> usize {
		self.timestamps.len()
	}

	fn is_empty(&self) -> bool {
		self.timestamps.is_empty()
	}

	fn set_max_len(&mut self, _len: usize) -> rustyline::Result<()> {
		// No-op: we don't clear audit logs through rustyline
		Ok(())
	}

	fn ignore_dups(&mut self, _yes: bool) -> rustyline::Result<()> {
		// No-op: we never ignore duplicates
		Ok(())
	}

	fn ignore_space(&mut self, _yes: bool) {
		// No-op: we never ignore entries
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
		// No-op: we don't clear audit logs
		Ok(())
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
				rustyline::error::ReadlineError::Io(std::io::Error::other(e.to_string()))
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
				rustyline::error::ReadlineError::Io(std::io::Error::other(e.to_string()))
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

/// Get active Tailscale human peers
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
	fn test_audit_roundtrip() {
		let temp_dir = tempfile::tempdir().unwrap();
		let db_path = temp_dir.path().join("test.redb");

		let mut audit = Audit::open_empty(db_path).unwrap();

		let mut state = ReplState {
			db_user: "dbuser".to_string(),
			sys_user: "testuser".to_string(),
			write_mode: false,
			ots: None,
			..ReplState::new()
		};
		audit.set_repl_state(&state);

		// Add some entries
		audit.add_entry("SELECT 1;".to_string()).unwrap();
		audit.add_entry("SELECT 2;".to_string()).unwrap();

		state.write_mode = true;
		state.ots = Some("John Doe".to_string());
		audit.set_repl_state(&state);
		audit.add_entry("INSERT INTO foo;".to_string()).unwrap();

		// List all entries
		let entries = audit.list().unwrap();
		assert_eq!(entries.len(), 3);
		assert_eq!(entries[0].1.query, "SELECT 1;");
		assert_eq!(entries[1].1.query, "SELECT 2;");
		assert_eq!(entries[2].1.query, "INSERT INTO foo;");
		assert_eq!(entries[2].1.writemode, true);
		assert_eq!(entries[2].1.db_user, "dbuser");
		assert_eq!(entries[2].1.sys_user, "testuser");
		assert_eq!(entries[2].1.ots, Some("John Doe".to_string()));
	}
}
