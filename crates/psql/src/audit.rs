use std::sync::{Arc, Mutex};

use redb::{Database, TableDefinition};

use crate::repl::ReplState;
pub use entry::AuditEntry;

mod database;
mod entry;
mod history;
mod index;
mod tailscale;

pub const HISTORY_TABLE: TableDefinition<'_, u64, &str> = TableDefinition::new("history");
pub const INDEX_TABLE: TableDefinition<'_, u64, u64> = TableDefinition::new("index");

/// Audit manager using redb for persistent storage
///
/// This struct is safe for use with concurrent writers. Multiple psql processes
/// can write to the same database simultaneously using redb's MVCC for consistency.
#[derive(Debug)]
pub struct Audit {
	pub(crate) db: Arc<Database>,
	/// State to record as context for new entries
	pub repl_state: Arc<Mutex<ReplState>>,
}
