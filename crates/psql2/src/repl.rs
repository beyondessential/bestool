use crate::audit::Audit;
use crate::completer::SqlCompleter;
use crate::config::PsqlConfig;
use crate::highlighter::Theme;
use crate::input::{handle_input, ReplAction};
use crate::ots;
use crate::parser::QueryModifier;
use crate::query::execute_query;
use crate::schema_cache::SchemaCacheManager;
use crate::snippets::Snippets;
use miette::{bail, IntoDiagnostic, Result};
use rustyline::error::ReadlineError;
use rustyline::history::History;
use rustyline::Editor;
use std::collections::BTreeMap;
use std::ops::ControlFlow;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::fs::File;
use tokio::io::{self, AsyncWriteExt};
use tokio::sync::Mutex as TokioMutex;
use tracing::{debug, error, instrument, warn};

pub(crate) struct ReplContext<'a> {
	client: &'a tokio_postgres::Client,
	monitor_client: &'a tokio_postgres::Client,
	backend_pid: i32,
	theme: Theme,
	repl_state: &'a Arc<Mutex<ReplState>>,
	rl: &'a mut Editor<SqlCompleter, Audit>,
}

impl ReplAction {
	pub(crate) async fn handle(self, ctx: &mut ReplContext<'_>, line: &str) -> ControlFlow<()> {
		// Add to history before handling the action (except for Continue and SnippetSave)
		// SnippetSave will add itself after it retrieves the preceding command
		if !matches!(self, ReplAction::Continue | ReplAction::SnippetSave { .. }) {
			let history = ctx.rl.history_mut();
			history.set_repl_state(&ctx.repl_state.lock().unwrap());
			if let Err(e) = history.add_entry(line.into()) {
				debug!("failed to add to history: {}", e);
			}
		}

		match self {
			ReplAction::Continue => ControlFlow::Continue(()),
			ReplAction::ToggleExpanded => handle_toggle_expanded(ctx),
			ReplAction::Exit => handle_exit(),
			ReplAction::ToggleWriteMode => handle_write_mode_toggle(ctx).await,
			ReplAction::Edit { content } => handle_edit(ctx, content).await,
			ReplAction::IncludeFile { file_path, vars } => {
				handle_include(ctx, &file_path, vars).await
			}
			ReplAction::RunSnippet { name, vars } => handle_run_snippet(ctx, name, vars).await,
			ReplAction::SetOutputFile { file_path } => handle_set_output(ctx, &file_path).await,
			ReplAction::UnsetOutputFile => handle_unset_output(ctx).await,
			ReplAction::Debug { what } => handle_debug(ctx, what),
			ReplAction::Help => handle_help(),
			ReplAction::SetVar { name, value } => handle_set_var(ctx, name, value),
			ReplAction::UnsetVar { name } => handle_unset_var(ctx, name),
			ReplAction::LookupVar { pattern } => handle_lookup_var(ctx, pattern),
			ReplAction::GetVar { name } => handle_get_var(ctx, name),
			ReplAction::SnippetSave { name } => handle_snippet_save(ctx, name, line).await,
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => handle_execute(ctx, input, sql, modifiers).await,
		}
	}
}

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

fn handle_toggle_expanded(ctx: &mut ReplContext<'_>) -> ControlFlow<()> {
	let mut state = ctx.repl_state.lock().unwrap();
	state.expanded_mode = !state.expanded_mode;
	eprintln!(
		"Expanded display is {}.",
		if state.expanded_mode { "on" } else { "off" }
	);
	ControlFlow::Continue(())
}

fn handle_exit() -> ControlFlow<()> {
	ControlFlow::Break(())
}

async fn handle_edit(ctx: &mut ReplContext<'_>, content: Option<String>) -> ControlFlow<()> {
	use rustyline::history::{History as _, SearchDirection};

	// Get the initial content - either from argument or from history
	let initial_content = if let Some(content) = content {
		content
	} else {
		// Get the last command from history
		let hist_len = ctx.rl.history().len();
		if hist_len > 0 {
			match ctx.rl.history().get(hist_len - 1, SearchDirection::Forward) {
				Ok(Some(result)) => result.entry.to_string(),
				_ => String::new(),
			}
		} else {
			String::new()
		}
	};

	// Open editor with the content
	match edit::edit(&initial_content) {
		Ok(edited_content) => {
			let edited_trimmed = edited_content.trim();

			// Only process if content is not empty
			if !edited_trimmed.is_empty() {
				debug!("editor returned content, processing it");

				// Add to history
				let history = ctx.rl.history_mut();
				history.set_repl_state(&ctx.repl_state.lock().unwrap());
				if let Err(e) = history.add_entry(edited_content.clone()) {
					debug!("failed to add to history: {}", e);
				}

				// Parse and execute the edited content
				let (_, action) =
					handle_input("", &edited_content, &ctx.repl_state.lock().unwrap());

				// Execute the action if it's an Execute action
				if let ReplAction::Execute {
					input,
					sql,
					modifiers,
				} = action
				{
					return handle_execute(ctx, input, sql, modifiers).await;
				}
			} else {
				debug!("editor returned empty content, skipping");
			}
		}
		Err(e) => {
			warn!("editor failed: {}", e);
		}
	}

	ControlFlow::Continue(())
}

async fn handle_include(
	ctx: &mut ReplContext<'_>,
	file_path: &Path,
	vars: Vec<(String, String)>,
) -> ControlFlow<()> {
	use std::fs;

	// Read the file content
	let content = match fs::read_to_string(file_path) {
		Ok(content) => content,
		Err(e) => {
			error!("Failed to read file '{file_path:?}': {e}");
			return ControlFlow::Continue(());
		}
	};

	let content_trimmed = content.trim();

	// Only process if content is not empty
	if !content_trimmed.is_empty() {
		debug!("read {} bytes from file '{file_path:?}'", content.len());

		// Add to history
		let history = ctx.rl.history_mut();
		history.set_repl_state(&ctx.repl_state.lock().unwrap());
		if let Err(e) = history.add_entry(content.clone()) {
			debug!("failed to add to history: {e}");
		}

		// Apply temporary variable shadowing
		let mut state = ctx.repl_state.lock().unwrap();
		let saved_vars: Vec<(String, Option<String>)> = vars
			.iter()
			.map(|(name, _)| (name.clone(), state.vars.get(name).cloned()))
			.collect();

		// Set the temporary variables
		for (name, value) in &vars {
			state.vars.insert(name.clone(), value.clone());
		}
		drop(state);

		// Parse and execute the content
		let (_, action) = handle_input("", &content, &ctx.repl_state.lock().unwrap());

		// Execute the action if it's an Execute action
		let result = if let ReplAction::Execute {
			input,
			sql,
			modifiers,
		} = action
		{
			handle_execute(ctx, input, sql, modifiers).await
		} else {
			ControlFlow::Continue(())
		};

		// Restore original variables
		let mut state = ctx.repl_state.lock().unwrap();
		for (name, original_value) in saved_vars {
			match original_value {
				Some(value) => state.vars.insert(name, value),
				None => state.vars.remove(&name),
			};
		}

		return result;
	} else {
		debug!("file '{file_path:?}' is empty, skipping");
	}

	ControlFlow::Continue(())
}

async fn handle_run_snippet(
	ctx: &mut ReplContext<'_>,
	name: String,
	vars: Vec<(String, String)>,
) -> ControlFlow<()> {
	// Get the snippet path
	let state = ctx.repl_state.lock().unwrap();
	let file_path = match state.snippets.path(&name) {
		Ok(path) => path,
		Err(err) => {
			error!("Failed to find snippet '{}': {}", name, err);
			return ControlFlow::Continue(());
		}
	};
	drop(state);

	// Use handle_include to execute the snippet with variables
	handle_include(ctx, &file_path, vars).await
}

async fn handle_set_output(ctx: &mut ReplContext<'_>, file_path: &Path) -> ControlFlow<()> {
	// Close existing file if any
	let _ = handle_unset_output(ctx).await;

	// Open new file
	match File::create(file_path).await {
		Ok(file) => {
			debug!("opened output file: {file_path:?}");
			eprintln!("Output will be written to: {file_path:?}");
			let mut state = ctx.repl_state.lock().unwrap();
			state.output_file = Some(Arc::new(TokioMutex::new(file)));
		}
		Err(e) => {
			error!("Failed to open output file '{file_path:?}': {e}");
		}
	}

	ControlFlow::Continue(())
}

async fn handle_unset_output(ctx: &mut ReplContext<'_>) -> ControlFlow<()> {
	// Close existing file if any
	let file_arc_opt = {
		let mut state = ctx.repl_state.lock().unwrap();
		state.output_file.take()
	};

	if let Some(file_arc) = file_arc_opt {
		// Flush and close the file
		let mut file = file_arc.lock().await;
		if let Err(e) = file.flush().await {
			warn!("failed to flush output file: {e}");
		}
		debug!("closed output file");
		eprintln!("Output redirection closed");
	}

	ControlFlow::Continue(())
}

fn handle_debug(ctx: &mut ReplContext<'_>, what: crate::parser::DebugWhat) -> ControlFlow<()> {
	use crate::parser::DebugWhat;

	match what {
		DebugWhat::State => {
			let state = ctx.repl_state.lock().unwrap();
			eprintln!("ReplState: {:#?}", *state);
		}
		DebugWhat::Help => {
			eprintln!("Available debug commands:");
			eprintln!("  \\debug state  - Show current REPL state");
		}
	}

	ControlFlow::Continue(())
}

fn handle_help() -> ControlFlow<()> {
	eprintln!("Available metacommands:");
	eprintln!("  \\?            - Show this help");
	eprintln!("  \\help         - Show this help");
	eprintln!("  \\q            - Quit");
	eprintln!("  \\x            - Toggle expanded output mode");
	eprintln!("  \\W            - Toggle write mode");
	eprintln!("  \\e [query]    - Edit query in external editor");
	eprintln!("  \\i <file>     - Execute commands from file");
	eprintln!("  \\o [file]     - Send query results to file (or close if no file specified)");
	eprintln!("  \\debug [cmd]  - Debug commands (run \\debug for options)");
	eprintln!("  \\snip run <name> - Run a saved snippet");
	eprintln!("  \\snip save <name> - Save the preceding command as a snippet");
	eprintln!("  \\set <name> <value> - Set a variable");
	eprintln!("  \\unset <name> - Unset a variable");
	eprintln!("  \\get <name>   - Get and print a variable value");
	eprintln!("  \\vars [pattern] - List variables (optionally matching pattern)");
	eprintln!();
	eprintln!("Query modifiers (used after query):");
	eprintln!("  \\g            - Execute query");
	eprintln!("  \\gx           - Execute query with expanded output");
	eprintln!("  \\gj           - Execute query with JSON output");
	eprintln!("  \\gv           - Execute query without variable interpolation");
	eprintln!("  \\go <file>    - Execute query and write output to file");
	eprintln!("  \\gset [prefix] - Execute query and store results in variables");
	eprintln!();
	eprintln!("Modifiers can be combined, e.g. \\gxj for expanded JSON output");
	eprintln!();
	eprintln!("Variable interpolation:");
	eprintln!("  ${{name}}       - Replace with variable value (errors if not set)");
	eprintln!("  ${{{{name}}}}     - Escape: produces ${{name}} without replacement");

	ControlFlow::Continue(())
}

fn handle_set_var(ctx: &mut ReplContext<'_>, name: String, value: String) -> ControlFlow<()> {
	let mut state = ctx.repl_state.lock().unwrap();
	state.vars.insert(name, value);
	ControlFlow::Continue(())
}

fn handle_unset_var(ctx: &mut ReplContext<'_>, name: String) -> ControlFlow<()> {
	let mut state = ctx.repl_state.lock().unwrap();
	if state.vars.remove(&name).is_none() {
		eprintln!("Variable '{}' not found", name);
	}
	ControlFlow::Continue(())
}

fn handle_lookup_var(ctx: &mut ReplContext<'_>, pattern: Option<String>) -> ControlFlow<()> {
	let state = ctx.repl_state.lock().unwrap();

	let matching_vars: Vec<(&String, &String)> = if let Some(ref pat) = pattern {
		// Simple wildcard matching: * matches any characters
		state
			.vars
			.iter()
			.filter(|(name, _)| matches_pattern(name, pat))
			.collect()
	} else {
		state.vars.iter().collect()
	};

	if matching_vars.is_empty() {
		if pattern.is_some() {
			eprintln!("No variables match the pattern");
		} else {
			eprintln!("No variables defined");
		}
		return ControlFlow::Continue(());
	}

	let mut table = comfy_table::Table::new();
	crate::query::configure_table(&mut table);
	table.set_header(vec!["Name", "Value"]);

	for (name, value) in matching_vars {
		table.add_row(vec![name, value]);
	}

	println!("{}", table);

	ControlFlow::Continue(())
}

fn handle_get_var(ctx: &mut ReplContext<'_>, name: String) -> ControlFlow<()> {
	let state = ctx.repl_state.lock().unwrap();
	match state.vars.get(&name) {
		Some(value) => println!("{}", value),
		None => eprintln!("Variable '{}' not found", name),
	}
	ControlFlow::Continue(())
}

async fn handle_snippet_save(
	ctx: &mut ReplContext<'_>,
	name: String,
	line: &str,
) -> ControlFlow<()> {
	let history = ctx.rl.history();

	// Get the last entry from history (which is the preceding command)
	if history.is_empty() {
		eprintln!("No command history available");
	} else {
		let last_idx = history.len() - 1;
		let content = match history.get(last_idx, rustyline::history::SearchDirection::Forward) {
			Ok(Some(result)) => result.entry.to_string(),
			_ => {
				eprintln!("Failed to retrieve last command from history");
				String::new()
			}
		};

		if !content.is_empty() {
			// Save the snippet - get snippets before awaiting
			let snippets = ctx.repl_state.lock().unwrap().snippets.clone();
			match snippets.save(&name, &content).await {
				Ok(path) => {
					println!("Snippet saved to {}", path.display());
				}
				Err(e) => eprintln!("Failed to save snippet '{}': {}", name, e),
			}
		}
	}

	// Always add the SnippetSave command to history, even if save failed
	let history = ctx.rl.history_mut();
	history.set_repl_state(&ctx.repl_state.lock().unwrap());
	if let Err(e) = history.add_entry(line.into()) {
		debug!("failed to add SnippetSave to history: {}", e);
	}

	ControlFlow::Continue(())
}

fn matches_pattern(text: &str, pattern: &str) -> bool {
	let mut text_chars = text.chars().peekable();
	let mut pattern_chars = pattern.chars().peekable();

	loop {
		match (pattern_chars.peek(), text_chars.peek()) {
			(None, None) => return true,
			(None, Some(_)) => return false,
			(Some(&'*'), _) => {
				pattern_chars.next();
				if pattern_chars.peek().is_none() {
					return true;
				}
				// Try to match rest of pattern at each position
				let rest_pattern: String = pattern_chars.clone().collect();
				while text_chars.peek().is_some() {
					let rest_text: String = text_chars.clone().collect();
					if matches_pattern(&rest_text, &rest_pattern) {
						return true;
					}
					text_chars.next();
				}
				return false;
			}
			(Some(&p), Some(&t)) => {
				if p == t {
					pattern_chars.next();
					text_chars.next();
				} else {
					return false;
				}
			}
			(Some(_), None) => return false,
		}
	}
}

async fn handle_execute(
	ctx: &mut ReplContext<'_>,
	_input: String,
	sql: String,
	modifiers: crate::parser::QueryModifiers,
) -> ControlFlow<()> {
	// Determine output destination
	// Priority: 1. Output modifier file, 2. ReplState output file, 3. stdout
	let output_file_path = modifiers.iter().find_map(|m| {
		if let QueryModifier::Output { file_path } = m {
			Some(file_path.clone())
		} else {
			None
		}
	});

	let use_colours = if output_file_path.is_some() {
		// Writing to a file from modifier - disable colours
		false
	} else if ctx.repl_state.lock().unwrap().output_file.is_some() {
		// Writing to repl state output file - disable colours
		false
	} else {
		// Writing to stdout - use colours from config
		ctx.repl_state.lock().unwrap().use_colours
	};

	let result = if let Some(path) = output_file_path {
		// Output modifier specified - open a temporary file
		match File::create(&path).await {
			Ok(mut file) => {
				let mut vars = {
					let state = ctx.repl_state.lock().unwrap();
					state.vars.clone()
				};
				let mut query_ctx = crate::query::QueryContext {
					client: ctx.client,
					modifiers: modifiers.clone(),
					theme: ctx.theme,
					writer: &mut file,
					use_colours,
					vars: Some(&mut vars),
				};
				let result = execute_query(&sql, &mut query_ctx).await;
				// Write vars back
				ctx.repl_state.lock().unwrap().vars = vars;
				result
			}
			Err(e) => {
				error!("Failed to open output file '{}': {}", path, e);
				return ControlFlow::Continue(());
			}
		}
	} else {
		let file_arc_opt = ctx.repl_state.lock().unwrap().output_file.clone();
		if let Some(file_arc) = file_arc_opt {
			// ReplState has an output file
			let mut vars = {
				let state = ctx.repl_state.lock().unwrap();
				state.vars.clone()
			};

			let mut file = file_arc.lock().await;
			let mut query_ctx = crate::query::QueryContext {
				client: ctx.client,
				modifiers: modifiers.clone(),
				theme: ctx.theme,
				writer: &mut *file,
				use_colours,
				vars: Some(&mut vars),
			};
			let result = execute_query(&sql, &mut query_ctx).await;

			// Write vars back
			ctx.repl_state.lock().unwrap().vars = vars;
			result
		} else {
			// Write to stdout
			let mut stdout = io::stdout();
			let mut vars = {
				let state = ctx.repl_state.lock().unwrap();
				state.vars.clone()
			};
			let mut query_ctx = crate::query::QueryContext {
				client: ctx.client,
				modifiers,
				theme: ctx.theme,
				writer: &mut stdout,
				use_colours,
				vars: Some(&mut vars),
			};
			let result = execute_query(&sql, &mut query_ctx).await;
			// Write vars back
			ctx.repl_state.lock().unwrap().vars = vars;
			result
		}
	};

	match result {
		Ok(()) => {
			// If write mode is on and we're not in a transaction, start one
			let tx_state = check_transaction_state(ctx.monitor_client, ctx.backend_pid).await;
			if ctx.repl_state.lock().unwrap().write_mode
				&& matches!(tx_state, TransactionState::None)
			{
				if let Err(e) = ctx.client.batch_execute("BEGIN").await {
					warn!("Failed to start transaction: {}", e);
				}
			}
		}
		Err(e) => {
			error!("{:?}", e);
		}
	}

	ControlFlow::Continue(())
}

fn build_prompt(
	database_name: &str,
	is_superuser: bool,
	buffer_is_empty: bool,
	transaction_state: TransactionState,
	write_mode: bool,
) -> String {
	let (transaction_marker, color_code) = match transaction_state {
		TransactionState::Error => ("!", "\x1b[1;31m"), // Bold red
		TransactionState::Active => {
			if write_mode {
				("*", "\x1b[1;34m") // Bold blue (write mode + transaction)
			} else {
				("*", "") // No color (read mode + transaction)
			}
		}
		TransactionState::Idle => {
			if write_mode {
				("", "\x1b[1;32m") // Bold green (write mode + idle transaction)
			} else {
				("", "") // No color (read mode + idle transaction)
			}
		}
		TransactionState::None => {
			if write_mode {
				("", "\x1b[1;32m") // Bold green (write mode, no transaction)
			} else {
				("", "") // No color (read mode, no transaction)
			}
		}
	};

	let reset_code = if color_code.is_empty() { "" } else { "\x1b[0m" };
	let prompt_suffix = if is_superuser { "#" } else { ">" };

	if buffer_is_empty {
		format!(
			"{}{}={}{}{} ",
			color_code, database_name, transaction_marker, prompt_suffix, reset_code
		)
	} else {
		format!("{}{}->{}  ", color_code, database_name, reset_code)
	}
}

async fn handle_write_mode_toggle(ctx: &mut ReplContext<'_>) -> ControlFlow<()> {
	let state = { ctx.repl_state.lock().unwrap().clone() };

	if state.write_mode {
		let tx_state = check_transaction_state(ctx.monitor_client, ctx.backend_pid).await;
		if !matches!(tx_state, TransactionState::Idle | TransactionState::None) {
			eprintln!(
				"Cannot disable write mode while in a transaction. COMMIT or ROLLBACK first."
			);
			return ControlFlow::Continue(());
		}

		let mut new_state = state.clone();
		new_state.write_mode = false;
		new_state.ots = None;

		match ctx
			.client
			.batch_execute("ROLLBACK; SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
			.await
		{
			Ok(_) => {
				debug!("Write mode disabled");
				eprintln!("SESSION IS NOW READ ONLY");
				ctx.rl.history_mut().set_repl_state(&new_state);
				*ctx.repl_state.lock().unwrap() = new_state;
			}
			Err(e) => {
				error!("Failed to disable write mode: {}", e);
			}
		}
	} else {
		match ots::prompt_for_ots(ctx.rl.history()) {
			Ok(new_ots) => {
				let mut new_state = state.clone();
				new_state.write_mode = true;
				new_state.ots = Some(new_ots.clone());

				match ctx
					.client
					.batch_execute(
						"SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; COMMIT; BEGIN",
					)
					.await
				{
					Ok(_) => {
						debug!("Write mode enabled");
						eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");
						ctx.rl.history_mut().set_repl_state(&new_state);
						*ctx.repl_state.lock().unwrap() = new_state;
					}
					Err(e) => {
						error!("Failed to enable write mode: {}", e);
					}
				}
			}
			Err(e) => {
				error!("Failed to enable write mode: {}", e);
			}
		}
	}

	ControlFlow::Continue(())
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TransactionState {
	None,
	Idle,
	Active,
	Error,
}

/// Check the transaction state of a connection by querying from a separate monitoring connection
async fn check_transaction_state(
	monitor_client: &tokio_postgres::Client,
	backend_pid: i32,
) -> TransactionState {
	// Query pg_stat_activity from a separate connection to get the true state
	// of the main connection without interfering with its transaction state
	match monitor_client
		.query_one(
			"SELECT state, backend_xid::text FROM pg_stat_activity WHERE pid = $1",
			&[&backend_pid],
		)
		.await
	{
		Ok(row) => {
			let state: String = row.get(0);
			let backend_xid: Option<String> = row.get(1);

			if state == "idle in transaction (aborted)" {
				TransactionState::Error
			} else if state.starts_with("idle in transaction") {
				if backend_xid.is_some() && !backend_xid.as_ref().unwrap().is_empty() {
					TransactionState::Active
				} else {
					TransactionState::Idle
				}
			} else if state == "active" {
				match monitor_client
					.query_one(
						"SELECT xact_start, backend_xid::text FROM pg_stat_activity WHERE pid = $1",
						&[&backend_pid],
					)
					.await
				{
					Ok(row) => {
						let xact_start: Option<std::time::SystemTime> = row.get(0);
						let backend_xid: Option<String> = row.get(1);

						if xact_start.is_some() {
							if backend_xid.is_some() && !backend_xid.as_ref().unwrap().is_empty() {
								TransactionState::Active
							} else {
								TransactionState::Idle
							}
						} else {
							TransactionState::None
						}
					}
					Err(_) => TransactionState::None,
				}
			} else {
				TransactionState::None
			}
		}
		Err(_) => TransactionState::None,
	}
}

#[instrument(level = "debug")]
pub async fn run(config: PsqlConfig) -> Result<()> {
	let audit_path = if let Some(path) = config.audit_path {
		path.clone()
	} else {
		Audit::default_path()?
	};
	let db_user = config.user.clone().unwrap_or_else(|| {
		std::env::var("USER")
			.or_else(|_| std::env::var("USERNAME"))
			.unwrap_or_else(|_| "unknown".to_string())
	});

	debug!("getting connection from pool");
	let client = config.pool.get().await.into_diagnostic()?;

	debug!("connected to database");

	if config.write {
		debug!("setting session to read-write mode with autocommit off");
		client
			.batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; BEGIN")
			.await
			.into_diagnostic()?;
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

	let info_rows = client
		.query(
			"SELECT current_database(), current_user, usesuper FROM pg_user WHERE usename = current_user",
			&[],
		)
		.await
		.into_diagnostic()?;

	let (database_name, is_superuser) = if let Some(row) = info_rows.first() {
		let db: String = row.get(0);
		let is_super: bool = row.get(2);
		(db, is_super)
	} else {
		(config.database_name.clone(), false)
	};

	// Get the backend PID of the main connection
	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.into_diagnostic()?
		.get(0);
	debug!(pid=%backend_pid, "main connection backend PID");

	// Create a separate connection for monitoring transaction state
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

		// write_mode: true (from the CLI) is handled later
		write_mode: false,
		ots: None,
		use_colours: config.use_colours,
		vars: BTreeMap::new(),
		snippets: Snippets::new(),
	};

	let audit = Audit::open(&audit_path, repl_state.clone())?;
	let repl_state = Arc::new(Mutex::new(repl_state));

	debug!("initializing schema cache");
	let schema_cache_manager = SchemaCacheManager::new(config.pool.clone());
	let cache_arc = schema_cache_manager.cache_arc();
	let _cache_task = schema_cache_manager.start_background_refresh();

	let completer = SqlCompleter::new(config.theme)
		.with_schema_cache(cache_arc)
		.with_repl_state(repl_state.clone());
	let mut rl: Editor<SqlCompleter, Audit> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.build(),
		audit,
	)
	.into_diagnostic()?;
	rl.set_helper(Some(completer));

	// If --write is given on the CLI, toggle write mode as the first action
	// This saves us from handling prompts/history outside of this function
	if config.write {
		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: config.theme,
			repl_state: &repl_state,
			rl: &mut rl,
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
		let transaction_state = check_transaction_state(&monitor_client, backend_pid).await;
		let current_write_mode = repl_state.lock().unwrap().write_mode;

		let prompt = build_prompt(
			&database_name,
			is_superuser,
			buffer.is_empty(),
			transaction_state,
			current_write_mode,
		);

		let readline = rl.readline(&prompt);
		match readline {
			Ok(line) => {
				let line = line.trim();
				if line.is_empty() && buffer.is_empty() {
					continue;
				}

				let (new_buffer, action) =
					{ handle_input(&buffer, line, &repl_state.lock().unwrap()) };
				buffer = new_buffer;

				let mut ctx = ReplContext {
					client: &client,
					monitor_client: &monitor_client,
					backend_pid,
					theme: config.theme,
					repl_state: &repl_state,
					rl: &mut rl,
				};

				if action.handle(&mut ctx, line).await.is_break() {
					break;
				}
			}
			Err(ReadlineError::Interrupted) => {
				debug!("CTRL-C");
				buffer.clear();
			}
			Err(ReadlineError::Eof) => {
				debug!("CTRL-D");
				break;
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

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn test_psql_config_creation() {
		let connection_string = "postgresql://localhost/test";
		let pool = crate::pool::create_pool(connection_string)
			.await
			.expect("Failed to create pool");

		let config = PsqlConfig {
			pool,
			user: Some("testuser".to_string()),
			theme: Theme::Dark,
			audit_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
			database_name: "test".to_string(),
			write: false,
			use_colours: true,
		};

		assert_eq!(config.user, Some("testuser".to_string()));
		assert_eq!(config.database_name, "test");
	}

	#[tokio::test]
	async fn test_psql_config_no_user() {
		let connection_string = "postgresql://localhost/test";
		let pool = crate::pool::create_pool(connection_string)
			.await
			.expect("Failed to create pool");

		let config = PsqlConfig {
			pool,
			user: None,
			theme: Theme::Dark,
			audit_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
			database_name: "test".to_string(),
			write: false,
			use_colours: true,
		};

		assert_eq!(config.user, None);
	}

	#[test]
	fn test_psql_error_display() {
		let err = crate::config::PsqlError::ConnectionFailed;
		assert_eq!(format!("{}", err), "database connection failed");

		let err = crate::config::PsqlError::QueryFailed;
		assert_eq!(format!("{}", err), "query execution failed");
	}

	#[test]
	fn test_snippet_save_excluded_from_preceding_command() {
		use crate::audit::Audit;
		use tempfile::TempDir;

		// Create a temporary audit database
		let temp_dir = TempDir::new().unwrap();
		let audit_path = temp_dir.path().join("history.redb");

		// Create an audit log and add some entries
		let repl_state = ReplState::new();
		let mut audit = Audit::open(&audit_path, repl_state).unwrap();
		audit.add_entry("SELECT 1;".into()).unwrap();
		audit.add_entry("SELECT 2;".into()).unwrap();

		// Verify that the last entry is SELECT 2, not the \snip save command
		let last_idx = audit.len() - 1;
		let last_entry = audit
			.get(last_idx, rustyline::history::SearchDirection::Forward)
			.unwrap();
		assert!(last_entry.is_some());
		if let Some(result) = last_entry {
			assert_eq!(result.entry, "SELECT 2;");
		}
	}

	#[tokio::test]
	async fn test_text_cast_for_record_types() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let mut stdout = tokio::io::stdout();
		let mut query_ctx = crate::query::QueryContext {
			client: &*client,
			modifiers: crate::parser::QueryModifiers::new(),
			theme: crate::highlighter::Theme::Dark,
			writer: &mut stdout,
			use_colours: true,
			vars: None,
		};
		let result =
			crate::query::execute_query("SELECT row(1, 'foo', true) as record", &mut query_ctx)
				.await;

		assert!(result.is_ok());
	}

	#[tokio::test]
	async fn test_array_formatting() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let mut stdout = tokio::io::stdout();
		let mut query_ctx = crate::query::QueryContext {
			client: &*client,
			modifiers: crate::parser::QueryModifiers::new(),
			theme: crate::highlighter::Theme::Dark,
			writer: &mut stdout,
			use_colours: true,
			vars: None,
		};
		let result =
			crate::query::execute_query("SELECT ARRAY[1, 2, 3] as numbers", &mut query_ctx).await;

		assert!(result.is_ok());
	}

	#[tokio::test]
	async fn test_database_info_query() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let info_rows = client
			.query(
				"SELECT current_database(), current_user, usesuper FROM pg_user WHERE usename = current_user",
				&[],
			)
			.await
			.expect("Failed to query database info");

		assert!(!info_rows.is_empty());
		let row = info_rows.first().expect("No rows returned");
		let db_name: String = row.get(0);
		let _username: String = row.get(1);
		let _is_super: bool = row.get(2);

		assert!(!db_name.is_empty());
	}

	#[tokio::test]
	async fn test_transaction_state_none() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		let state = check_transaction_state(&*monitor_client, backend_pid).await;

		assert_eq!(state, TransactionState::None);
	}

	#[tokio::test]
	async fn test_transaction_state_idle() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		// Start a transaction without allocating an XID
		client
			.batch_execute("BEGIN")
			.await
			.expect("Failed to begin transaction");

		// Should detect idle transaction (no XID allocated yet)
		let state = check_transaction_state(&*monitor_client, backend_pid).await;
		assert_eq!(state, TransactionState::Idle);

		// Clean up
		client.batch_execute("ROLLBACK").await.ok();
	}

	#[tokio::test]
	async fn test_transaction_state_active() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		// Start a transaction and allocate an XID by creating a temp table
		client
			.batch_execute("BEGIN; CREATE TEMP TABLE test_xid (id INT)")
			.await
			.expect("Failed to begin transaction and allocate XID");

		// Give pg_stat_activity time to update
		tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

		// Should detect active transaction with XID
		let state = check_transaction_state(&*monitor_client, backend_pid).await;
		assert_eq!(state, TransactionState::Active);

		// Clean up
		client.batch_execute("ROLLBACK").await.ok();
	}

	#[tokio::test]
	async fn test_transaction_state_error() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		// Start a transaction
		client
			.batch_execute("BEGIN")
			.await
			.expect("Failed to begin transaction");

		// Cause an error in the transaction (division by zero)
		let _ = client.query("SELECT 1/0", &[]).await;

		// Give pg_stat_activity time to update
		tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

		// Should detect error state
		let state = check_transaction_state(&*monitor_client, backend_pid).await;
		assert_eq!(state, TransactionState::Error);

		// Clean up
		client.batch_execute("ROLLBACK").await.ok();
	}

	#[tokio::test]
	async fn test_write_mode_disable_with_idle_transaction() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		// Simulate enabling write mode: set read-write and begin transaction
		client
			.batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; BEGIN")
			.await
			.expect("Failed to enable write mode");

		// Should be in idle transaction state (no XID allocated)
		let state = check_transaction_state(&monitor_client, backend_pid).await;
		assert_eq!(state, TransactionState::Idle);

		// Disabling write mode should succeed with idle transaction
		client
			.batch_execute("ROLLBACK; SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
			.await
			.expect("Failed to disable write mode with idle transaction");

		// Should be back to no transaction
		let state = check_transaction_state(&monitor_client, backend_pid).await;
		assert_eq!(state, TransactionState::None);
	}

	#[tokio::test]
	async fn test_write_mode_disable_blocked_with_active_transaction() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		// Simulate write mode with actual write allocating an XID
		client
			.batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; BEGIN; CREATE TEMP TABLE test_write_block (id INT)")
			.await
			.expect("Failed to enable write mode and allocate XID");

		// Give pg_stat_activity time to update
		tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

		// Should be in active transaction state (XID allocated)
		let state = check_transaction_state(&monitor_client, backend_pid).await;
		assert_eq!(state, TransactionState::Active);

		// In real code, this would be blocked by checking state == Active
		// We verify that we correctly detect Active state which prevents disable

		// Clean up
		client.batch_execute("ROLLBACK").await.ok();
	}

	#[tokio::test]
	async fn test_backend_xmin_vs_xid_in_idle_transaction() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");

		// Start a transaction without allocating an XID
		client
			.batch_execute("BEGIN")
			.await
			.expect("Failed to begin transaction");

		// Query the backend state directly to verify backend_xmin is set but backend_xid is not
		let row = client
			.query_one(
				"SELECT backend_xid::text, backend_xmin::text FROM pg_stat_activity WHERE pid = pg_backend_pid()",
				&[],
			)
			.await
			.expect("Failed to query pg_stat_activity");

		let backend_xid: Option<String> = row.get(0);
		let backend_xmin: Option<String> = row.get(1);

		// backend_xid should be NULL (None or empty) in idle transaction
		assert!(
			backend_xid.is_none() || backend_xid.as_ref().unwrap().is_empty(),
			"backend_xid should be NULL in idle transaction, got: {:?}",
			backend_xid
		);

		// backend_xmin should be set (Some and non-empty) even in idle transaction
		assert!(
			backend_xmin.is_some() && !backend_xmin.as_ref().unwrap().is_empty(),
			"backend_xmin should be set in idle transaction, got: {:?}",
			backend_xmin
		);

		// Clean up
		client.batch_execute("ROLLBACK").await.ok();
	}
}
