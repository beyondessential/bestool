use miette::{IntoDiagnostic, Result};
use redb::{ReadableDatabase, ReadableTable};
use serde::{Deserialize, Serialize};
use tracing::{instrument, trace};

use crate::repl::ReplState;

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
	pub tailscale: Vec<super::tailscale::TailscalePeer>,
	/// OTS (Over The Shoulder) value for write mode sessions
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub ots: Option<String>,
}

impl super::Audit {
	/// Set the context for new history entries from REPL state
	#[instrument(level = "debug")]
	pub fn set_repl_state(&mut self, repl_state: &ReplState) {
		self.repl_state = repl_state.clone();
	}

	/// Get entry by timestamp
	///
	/// Returns None if the entry doesn't exist (may have been deleted by another process)
	pub(crate) fn get_entry(&self, timestamp: u64) -> Result<Option<AuditEntry>> {
		let read_txn = self.db.begin_read().into_diagnostic()?;

		let table = match read_txn.open_table(super::HISTORY_TABLE) {
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

	/// Add a new entry to the audit
	pub fn add_entry(&mut self, query: String) -> Result<()> {
		trace!("adding audit entry");
		let tailscale = super::tailscale::get_active_peers()
			.ok()
			.unwrap_or_default();

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
			let mut table = write_txn
				.open_table(super::HISTORY_TABLE)
				.into_diagnostic()?;
			table.insert(timestamp, json.as_str()).into_diagnostic()?;
		}
		write_txn.commit().into_diagnostic()?;

		self.timestamps.push(timestamp);

		Ok(())
	}

	/// Get all audit entries in chronological order (oldest first)
	pub fn list(&self) -> Result<Vec<(u64, AuditEntry)>> {
		let read_txn = self.db.begin_read().into_diagnostic()?;
		let table = read_txn
			.open_table(super::HISTORY_TABLE)
			.into_diagnostic()?;

		let mut entries = Vec::new();
		for item in table.iter().into_diagnostic()? {
			let (timestamp, json) = item.into_diagnostic()?;
			let entry: AuditEntry = serde_json::from_str(json.value()).into_diagnostic()?;
			entries.push((timestamp.value(), entry));
		}

		Ok(entries)
	}
}

#[cfg(test)]
mod tests {
	use crate::audit::*;

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
		assert!(entries[2].1.writemode);
		assert_eq!(entries[2].1.db_user, "dbuser");
		assert_eq!(entries[2].1.sys_user, "testuser");
		assert_eq!(entries[2].1.ots, Some("John Doe".to_string()));
	}
}
