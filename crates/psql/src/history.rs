//! Query history storage using redb.
//!
//! History entries are stored with timestamp keys and JSON-serialized values
//! containing the query, user, and write mode information.

use miette::{IntoDiagnostic, Result};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
	#[serde(skip_serializing_if = "Vec::is_empty")]
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
}

impl History {
	/// Open or create a history database at the given path
	pub fn open(path: PathBuf) -> Result<Self> {
		let db = Database::create(&path).into_diagnostic()?;
		Ok(Self { db })
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

	/// Add a new entry to the history
	pub fn add(
		&self,
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
	pub fn clear(&self) -> Result<()> {
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
		Ok(())
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

		let history = History::open(db_path).unwrap();

		// Add some entries
		history
			.add(
				"SELECT 1;".to_string(),
				"dbuser".to_string(),
				"testuser".to_string(),
				false,
			)
			.unwrap();
		history
			.add(
				"SELECT 2;".to_string(),
				"dbuser".to_string(),
				"testuser".to_string(),
				false,
			)
			.unwrap();
		history
			.add(
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
}
