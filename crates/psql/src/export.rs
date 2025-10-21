//! Export utilities for history entries.
//!
//! This module provides common types and functions for exporting history
//! entries to JSON format with RFC3339 timestamps.

use crate::history::{HistoryEntry, TailscalePeer};
use serde::Serialize;

/// Export entry format with RFC3339 timestamp
#[derive(Debug, Serialize)]
pub struct ExportEntry {
	pub ts: String,
	pub query: String,
	pub db_user: String,
	pub sys_user: String,
	pub writemode: bool,
	#[serde(skip_serializing_if = "Vec::is_empty")]
	pub tailscale: Vec<TailscalePeer>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub ots: Option<String>,
}

impl ExportEntry {
	/// Create an export entry from a timestamp (microseconds) and history entry
	pub fn from_history(timestamp_micros: u64, entry: HistoryEntry) -> Self {
		Self {
			ts: timestamp_to_rfc3339(timestamp_micros),
			query: entry.query,
			db_user: entry.db_user,
			sys_user: entry.sys_user,
			writemode: entry.writemode,
			tailscale: entry.tailscale,
			ots: entry.ots,
		}
	}
}

/// Convert microseconds since epoch to RFC3339 timestamp string
pub fn timestamp_to_rfc3339(micros: u64) -> String {
	use jiff::Timestamp;

	let secs = (micros / 1_000_000) as i64;
	let nanos = ((micros % 1_000_000) * 1_000) as i32;

	Timestamp::new(secs, nanos)
		.map(|ts| ts.to_string())
		.unwrap_or_else(|_| format!("invalid-timestamp-{}", micros))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_timestamp_to_rfc3339() {
		// Test a known timestamp: 2024-01-15 13:10:45.123456 UTC
		// = 1705324245123456 microseconds since epoch
		let micros = 1705324245123456;
		let rfc3339 = timestamp_to_rfc3339(micros);

		assert_eq!(rfc3339, "2024-01-15T13:10:45.123456Z");
	}

	#[test]
	fn test_export_entry_serialization() {
		let entry = ExportEntry {
			ts: "2024-01-15T12:30:45.123456Z".to_string(),
			query: "SELECT * FROM users;".to_string(),
			db_user: "postgres".to_string(),
			sys_user: "alice".to_string(),
			writemode: false,
			tailscale: vec![],
			ots: None,
		};

		let json = serde_json::to_string(&entry).unwrap();

		// Should be compact (single line)
		assert!(!json.contains('\n'));

		// Should contain expected fields
		assert!(json.contains("\"ts\""));
		assert!(json.contains("\"query\""));
		assert!(json.contains("\"db_user\""));
		assert!(json.contains("\"sys_user\""));
		assert!(json.contains("\"writemode\""));

		// Should NOT contain empty tailscale or null ots (due to skip_serializing_if)
		assert!(!json.contains("\"tailscale\""));
		assert!(!json.contains("\"ots\""));
	}

	#[test]
	fn test_export_entry_with_ots() {
		let entry = ExportEntry {
			ts: "2024-01-15T12:30:45.123456Z".to_string(),
			query: "INSERT INTO logs VALUES (1);".to_string(),
			db_user: "postgres".to_string(),
			sys_user: "alice".to_string(),
			writemode: true,
			tailscale: vec![],
			ots: Some("bob-watching".to_string()),
		};

		let json = serde_json::to_string(&entry).unwrap();

		// Should contain ots when present
		assert!(json.contains("\"ots\""));
		assert!(json.contains("bob-watching"));
	}

	#[test]
	fn test_from_history() {
		let history_entry = HistoryEntry {
			query: "SELECT 1;".to_string(),
			db_user: "testdb".to_string(),
			sys_user: "testuser".to_string(),
			writemode: false,
			tailscale: vec![],
			ots: None,
		};

		let export = ExportEntry::from_history(1705324245123456, history_entry);

		assert_eq!(export.ts, "2024-01-15T13:10:45.123456Z");
		assert_eq!(export.query, "SELECT 1;");
		assert_eq!(export.db_user, "testdb");
		assert_eq!(export.sys_user, "testuser");
		assert!(!export.writemode);
		assert!(export.tailscale.is_empty());
		assert!(export.ots.is_none());
	}
}
