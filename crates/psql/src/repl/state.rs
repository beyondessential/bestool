use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};

use rustyline::Editor;
use tokio::{fs::File, sync::Mutex as TokioMutex};

use crate::{
	audit::Audit, completer::SqlCompleter, pool::PgPool, snippets::Snippets, theme::Theme,
};

#[derive(Debug, Clone)]
pub struct ReplState {
	pub(crate) db_user: String,
	pub(crate) sys_user: String,
	pub(crate) expanded_mode: bool,
	pub(crate) write_mode: bool,
	pub(crate) ots: Option<String>,
	pub(crate) output_file: Option<Arc<TokioMutex<File>>>,
	pub(crate) use_colours: bool,
	pub(crate) vars: BTreeMap<String, String>,
	pub(crate) snippets: Snippets,
}

impl ReplState {
	#[cfg(test)]
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
