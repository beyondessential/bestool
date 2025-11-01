use std::sync::{Arc, Mutex};

use redb::{Database, TableDefinition};

use crate::repl::ReplState;
pub use entry::AuditEntry;

mod database;
mod entry;
mod history;
mod tailscale;

pub const HISTORY_TABLE: TableDefinition<'_, u64, &str> = TableDefinition::new("history");

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
	pub repl_state: Arc<Mutex<ReplState>>,
}
