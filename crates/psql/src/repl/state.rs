use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
	time::Instant,
};

use bestool_postgres::pool::PgPool;
use rustyline::Editor;
use tokio::{fs::File, sync::Mutex as TokioMutex};

use crate::{
	Config, audit::Audit, completer::SqlCompleter, result_store::ResultStore,
	schema_cache::SchemaCacheManager, snippets::Snippets,
};

use super::TransactionState;

#[derive(Debug, Clone)]
pub struct ReplState {
	pub config: Arc<Config>,
	pub db_user: String,
	pub sys_user: String,
	pub expanded_mode: bool,
	pub write_mode: bool,
	pub redact_mode: bool,
	pub ots: Option<String>,
	pub output_file: Option<Arc<TokioMutex<File>>>,
	pub vars: BTreeMap<String, String>,
	pub snippets: Snippets,
	pub transaction_state: TransactionState,
	pub result_store: ResultStore,
	pub from_snippet_or_include: bool,
	pub initial_content: Option<String>,
	/// Buffer the last `\e` invocation produced, kept only while `\e`s run
	/// back-to-back. A repeated `\e` reopens it so you can keep refining the
	/// same text; any other command clears it, so the next `\e` starts empty.
	pub last_edit_content: Option<String>,
	/// When write mode was last considered "active" — either toggled on, or
	/// just after a successful query while in write mode. Used by the idle
	/// timeout watcher to decide when to revert write mode to read-only.
	/// `None` whenever write mode is off.
	pub write_mode_active_at: Option<Instant>,
}

impl Default for ReplState {
	fn default() -> Self {
		Self::new()
	}
}

impl ReplState {
	pub fn new() -> Self {
		Self {
			config: Arc::new(Config::default()),
			db_user: "testuser".to_string(),
			sys_user: "localuser".to_string(),
			expanded_mode: false,
			write_mode: false,
			redact_mode: false,
			ots: None,
			output_file: None,
			vars: BTreeMap::new(),
			snippets: Snippets::empty(),
			transaction_state: TransactionState::None,
			result_store: ResultStore::new(),
			from_snippet_or_include: false,
			initial_content: None,
			last_edit_content: None,
			write_mode_active_at: None,
		}
	}
}

pub(crate) struct ReplContext<'a> {
	pub config: &'a Arc<Config>,
	pub client: &'a tokio_postgres::Client,
	pub monitor_client: &'a tokio_postgres::Client,
	pub backend_pid: i32,
	pub repl_state: &'a Arc<Mutex<ReplState>>,
	pub rl: &'a mut Editor<SqlCompleter, Audit>,
	pub pool: &'a PgPool,
	pub schema_cache_manager: &'a SchemaCacheManager,
	pub redact_mode: bool,
}
