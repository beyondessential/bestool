use miette::{IntoDiagnostic, Result};
use redb::{ReadableDatabase, ReadableTable};
use serde::{Deserialize, Serialize};
use tracing::trace;
use uuid::Uuid;

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
	/// Instance ID (working database UUID) for tracking which instance recorded this entry
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub instance_id: Option<Uuid>,
}

impl super::Audit {
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

		let state = self.repl_state.lock().unwrap();
		let instance_id = self.working_info.as_ref().map(|info| info.uuid);
		let entry = AuditEntry {
			query,
			db_user: state.db_user.clone(),
			sys_user: state.sys_user.clone(),
			writemode: state.write_mode,
			tailscale,
			ots: state.ots.clone(),
			instance_id,
		};
		drop(state);

		let json = serde_json::to_string(&entry).into_diagnostic()?;
		let timestamp = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.into_diagnostic()?
			.as_micros() as u64;

		let write_txn = self.db.begin_write().into_diagnostic()?;
		{
			let mut history_table = write_txn
				.open_table(super::HISTORY_TABLE)
				.into_diagnostic()?;
			history_table
				.insert(timestamp, json.as_str())
				.into_diagnostic()?;
		}
		write_txn.commit().into_diagnostic()?;

		// Add to index table
		self.hist_index_push(timestamp)?;

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

		// Add some entries
		audit.add_entry("SELECT 1;".to_string()).unwrap();
		audit.add_entry("SELECT 2;".to_string()).unwrap();

		{
			let mut state = audit.repl_state.lock().unwrap();
			state.db_user = "dbuser".to_string();
			state.sys_user = "testuser".to_string();
			state.write_mode = true;
			state.ots = Some("John Doe".to_string());
		}
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
		// In test mode (open_empty), there's no working_info, so instance_id should be None
		assert_eq!(entries[0].1.instance_id, None);
		assert_eq!(entries[1].1.instance_id, None);
		assert_eq!(entries[2].1.instance_id, None);
	}

	#[test]
	fn test_audit_instance_id() {
		let temp_dir = tempfile::tempdir().unwrap();
		// Use a subdirectory to avoid any existing psql_history
		let db_dir = temp_dir.path().join("audit_dir");
		std::fs::create_dir(&db_dir).unwrap();

		// Temporarily set HOME to temp dir to avoid importing real psql_history
		let _guard = temp_env::with_var("HOME", Some(db_dir.to_str().unwrap()), || {
			// Open in multi-process mode (using open instead of open_empty)
			let mut audit = Audit::open(
				&db_dir,
				std::sync::Arc::new(std::sync::Mutex::new(crate::repl::ReplState::new())),
			)
			.unwrap();

			// Add an entry
			audit.add_entry("SELECT 1;".to_string()).unwrap();

			// List entries
			let entries = audit.list().unwrap();
			assert_eq!(entries.len(), 1);

			// In multi-process mode, instance_id should be set to the working database UUID
			assert!(entries[0].1.instance_id.is_some());
			// Just verify it's a valid UUID (no need to check string length)
		});
	}
}
