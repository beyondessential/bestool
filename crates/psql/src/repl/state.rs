use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
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
	/// Whether queries are being executed from a snippet or include (don't recall in history)
	pub from_snippet_or_include: bool,
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
	/// Whether the current execution is from a snippet or include (don't recall in history)
	pub from_snippet_or_include: bool,
}
