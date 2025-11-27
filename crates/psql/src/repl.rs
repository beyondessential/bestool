use std::{collections::BTreeMap, ops::ControlFlow, sync::Arc};

use bestool_postgres::pool::PgPool;
use miette::{IntoDiagnostic, Result, bail, miette};
use rustyline::{
	Cmd, Editor, EventHandler, KeyEvent, config::CompletionType, error::ReadlineError,
};
use std::sync::Mutex;
use tracing::{debug, instrument, warn};

use crate::{
	audit::Audit,
	completer::SqlCompleter,
	config::Config,
	input::{ReplAction, handle_input},
	result_store::ResultStore,
	schema_cache::SchemaCacheManager,
	snippets::Snippets,
};

pub(crate) use state::ReplContext;
pub use state::ReplState;
pub use transaction::TransactionState;

mod copy;
mod debug;
mod describe;
mod edit;
mod execute;
mod exit;
mod expanded;
mod help;
mod include;
mod list;
mod output;
mod prompt;
mod result;
mod snippets;
mod state;
mod transaction;
mod vars;
mod write_mode;

#[cfg(test)]
mod tests;

impl ReplAction {
	pub(crate) async fn handle(self, ctx: &mut ReplContext<'_>, line: &str) -> ControlFlow<()> {
		if !matches!(self, ReplAction::SnippetSave { .. }) {
			let history = ctx.rl.history_mut();
			if let Err(e) = history.add_entry(line.into()) {
				debug!("failed to add to history: {e}");
			}
		}

		match self {
			ReplAction::ToggleExpanded => expanded::handle_toggle_expanded(ctx),
			ReplAction::Exit => exit::handle_exit(ctx).await,
			ReplAction::ToggleWriteMode => write_mode::handle_write_mode_toggle(ctx).await,
			ReplAction::Edit => edit::handle_edit(ctx).await,
			ReplAction::Copy => copy::handle_copy(),
			ReplAction::IncludeFile { file_path, vars } => {
				include::handle_include(ctx, &file_path, vars).await
			}
			ReplAction::RunSnippet { name, vars } => {
				snippets::handle_run_snippet(ctx, name, vars).await
			}
			ReplAction::SetOutputFile { file_path } => {
				output::handle_set_output(ctx, &file_path).await
			}
			ReplAction::UnsetOutputFile => output::handle_unset_output(ctx).await,
			ReplAction::Debug { what } => debug::handle_debug(ctx, what).await,
			ReplAction::Help => help::handle_help(),
			ReplAction::SetVar { name, value } => vars::handle_set_var(ctx, name, value),
			ReplAction::UnsetVar { name } => vars::handle_unset_var(ctx, name),
			ReplAction::LookupVar { pattern } => vars::handle_lookup_var(ctx, pattern),
			ReplAction::GetVar { name } => vars::handle_get_var(ctx, name),
			ReplAction::SnippetSave { name } => {
				snippets::handle_snippet_save(ctx, name, line).await
			}
			ReplAction::List {
				item,
				pattern,
				detail,
				sameconn,
			} => list::handle_list(ctx, item, pattern, detail, sameconn).await,
			ReplAction::Describe {
				item,
				detail,
				sameconn,
			} => describe::handle_describe(ctx, item, detail, sameconn).await,
			ReplAction::Result { subcommand } => result::handle_result(ctx, subcommand).await,
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => execute::handle_execute(ctx, input, sql, modifiers).await,
		}
	}
}

#[instrument(level = "debug")]
pub async fn run(pool: PgPool, config: Config) -> Result<()> {
	let audit_path = if let Some(path) = config.audit_path {
		path.clone()
	} else {
		Audit::default_path()?
	};

	debug!("getting connection from pool");
	let client = pool.get().await.into_diagnostic()?;

	if config.write {
		debug!("setting session to read-write mode");
		client
			.execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE", &[])
			.await
			.into_diagnostic()?;
		debug!("opening transaction");
		client.execute("BEGIN", &[]).await.into_diagnostic()?;
	} else {
		debug!("setting session to read-only mode");
		client
			.execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY", &[])
			.await
			.into_diagnostic()?;
	}

	debug!("executing version query");
	let rows = client
		.query("SELECT version();", &[])
		.await
		.into_diagnostic()?;

	if let Some(row) = rows.first() {
		let version: String = row.get(0);
		println!("{}", version);
	}

	let (database_name, db_user, is_superuser): (String, String, bool) = {
		let info_res = client
			.query(
				"SELECT current_database(), current_user, usesuper FROM pg_user WHERE usename = current_user",
				&[],
			)
			.await
			.into_diagnostic()?;
		let info = info_res
			.first()
			.ok_or_else(|| miette!("Unable to fetch connection information"))?;
		(info.get(0), info.get(1), info.get(2))
	};

	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.into_diagnostic()?
		.get(0);
	debug!(pid=%backend_pid, "main connection backend PID");

	debug!("getting monitor connection from pool");
	let monitor_client = config.pool.get().await.into_diagnostic()?;
	debug!("monitor connection established");

	let sys_user = std::env::var("USER")
		.or_else(|_| std::env::var("USERNAME"))
		.unwrap_or_else(|_| "unknown".to_string());

	let repl_state = ReplState {
		output_file: None,
		sys_user,
		db_user,
		expanded_mode: false,
		write_mode: false,
		ots: None,
		use_colours: config.use_colours,
		vars: BTreeMap::new(),
		snippets: Snippets::new(),
		transaction_state: TransactionState::None,
		result_store: ResultStore::new(),
	};

	let repl_state = Arc::new(Mutex::new(repl_state));
	let audit = Audit::open(&audit_path, Arc::clone(&repl_state))?;

	debug!("initializing schema cache");
	let schema_cache_manager = SchemaCacheManager::new(pool.clone());

	// Refresh schema cache on startup for column extraction
	debug!("refreshing schema cache on startup");
	if let Err(e) = schema_cache_manager.refresh().await {
		warn!("failed to refresh schema cache on startup: {}", e);
	}

	let cache_arc = schema_cache_manager.cache_arc();

	let completer = SqlCompleter::new(config.theme)
		.with_schema_cache(cache_arc)
		.with_repl_state(repl_state.clone());
	let mut rl: Editor<SqlCompleter, Audit> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.completion_type(CompletionType::List)
			.build(),
		audit,
	)
	.into_diagnostic()?;
	rl.set_helper(Some(completer));

	// Bind Alt+Enter to insert a literal newline
	rl.bind_sequence(KeyEvent::alt('\r'), EventHandler::Simple(Cmd::Newline));

	if config.write {
		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: config.theme,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
			redact_mode: config.redact_mode,
			redactions: &config.redactions,
		};

		if ReplAction::ToggleWriteMode
			.handle(&mut ctx, "")
			.await
			.is_break()
		{
			bail!("Write mode aborted");
		}
	}

	let mut buffer = String::new();

	loop {
		let transaction_state = TransactionState::check(&monitor_client, backend_pid).await;

		// Update transaction state in ReplState so the highlighter can access it
		{
			let mut state = repl_state.lock().unwrap();
			state.transaction_state = transaction_state;
		}

		let prompt = prompt::build_prompt(
			&database_name,
			is_superuser,
			buffer.is_empty(),
			transaction_state,
		);

		let readline = rl.readline(&prompt);
		match readline {
			Ok(line) => {
				let line = line.trim();
				if line.is_empty() && buffer.is_empty() {
					continue;
				}

				let (new_buffer, actions) =
					{ handle_input(&buffer, line, &repl_state.lock().unwrap()) };
				buffer = new_buffer;

				let mut ctx = ReplContext {
					client: &client,
					monitor_client: &monitor_client,
					backend_pid,
					theme: config.theme,
					repl_state: &repl_state,
					rl: &mut rl,
					pool: &pool,
					schema_cache_manager: &schema_cache_manager,
					redact_mode: config.redact_mode,
					redactions: &config.redactions,
				};

				// Handle all actions
				let mut should_exit = false;
				if actions.is_empty() {
					// No actions to execute, but still add to history
					let history = ctx.rl.history_mut();
					if let Err(e) = history.add_entry(line.into()) {
						debug!("failed to add to history: {e}");
					}
				} else {
					for action in actions {
						if action.handle(&mut ctx, line).await.is_break() {
							should_exit = true;
							break;
						}
					}
				}

				if should_exit {
					break;
				}
			}
			Err(ReadlineError::Interrupted) => {
				debug!("CTRL-C");
				buffer.clear();
			}
			Err(ReadlineError::Eof) => {
				debug!("CTRL-D");
				let mut ctx = ReplContext {
					client: &client,
					monitor_client: &monitor_client,
					backend_pid,
					theme: config.theme,
					repl_state: &repl_state,
					rl: &mut rl,
					pool: &pool,
					schema_cache_manager: &schema_cache_manager,
					redact_mode: config.redact_mode,
					redactions: &config.redactions,
				};

				if exit::handle_exit(&mut ctx).await.is_break() {
					break;
				}
			}
			Err(err) => {
				eprintln!("Error: {:?}", err);
				break;
			}
		}
	}

	rl.history_mut().compact()?;
	Ok(())
}
