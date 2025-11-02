use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};

use rustyline::Editor;
use tokio::{fs::File, sync::Mutex as TokioMutex};

use crate::{
	audit::Audit, completer::SqlCompleter, pool::PgPool, result_store::ResultStore,
	snippets::Snippets, theme::Theme,
};

use super::TransactionState;

#[derive(Debug, Clone)]
pub struct ReplState {
	pub db_user: String,
	pub sys_user: String,
	pub expanded_mode: bool,
	pub write_mode: bool,
	pub ots: Option<String>,
	pub output_file: Option<Arc<TokioMutex<File>>>,
	pub use_colours: bool,
	pub vars: BTreeMap<String, String>,
	pub snippets: Snippets,
	pub transaction_state: TransactionState,
	pub result_store: ResultStore,
}

impl ReplState {
	pub fn new() -> Self {
		Self {
			db_user: "testuser".to_string(),
			sys_user: "localuser".to_string(),
			expanded_mode: false,
			write_mode: false,
			ots: None,
			output_file: None,
			use_colours: true,
			vars: BTreeMap::new(),
			snippets: Snippets::empty(),
			transaction_state: TransactionState::None,
			result_store: ResultStore::new(),
		}
	}
}

pub(crate) struct ReplContext<'a> {
	pub client: &'a tokio_postgres::Client,
	pub monitor_client: &'a tokio_postgres::Client,
	pub backend_pid: i32,
	pub theme: Theme,
	pub repl_state: &'a Arc<Mutex<ReplState>>,
	pub rl: &'a mut Editor<SqlCompleter, Audit>,
	pub pool: &'a PgPool,
}
