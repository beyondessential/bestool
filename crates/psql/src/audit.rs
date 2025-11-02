use std::{
	sync::{Arc, Mutex},
	thread::JoinHandle,
};

use redb::{Database, TableDefinition};
use tracing::error;

use crate::repl::ReplState;
pub use entry::AuditEntry;
pub use multi_process::WorkingDatabase;

mod database;
mod entry;
mod history;
mod index;
mod multi_process;
mod tailscale;

pub const HISTORY_TABLE: TableDefinition<'_, u64, &str> = TableDefinition::new("history");
pub const INDEX_TABLE: TableDefinition<'_, u64, u64> = TableDefinition::new("index");

/// Audit manager using redb for persistent storage
pub struct Audit {
	pub(crate) db: Arc<Database>,
	/// State to record as context for new entries
	pub repl_state: Arc<Mutex<ReplState>>,
	/// Information about the working database (if using multi-process mode)
	pub(crate) working_info: Option<Arc<WorkingDatabase>>,
	/// Background sync thread handle (wrapped in Mutex for interior mutability in shutdown)
	pub(crate) sync_thread: Option<Mutex<Option<JoinHandle<()>>>>,
}

impl Drop for Audit {
	fn drop(&mut self) {
		if let Err(e) = self.shutdown() {
			error!("error during audit database shutdown: {}", e);
		}
	}
}
