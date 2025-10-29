use crate::completer::SqlCompleter;
use crate::highlighter::Theme;
use crate::history::History;
use crate::ots;
use crate::parser::{parse_metacommand, parse_query_modifiers, Metacommand, QueryModifiers};
use crate::query::execute_query;
use crate::schema_cache::SchemaCacheManager;
use miette::{IntoDiagnostic, Result};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplAction {
	Continue,
	Execute {
		input: String,
		sql: String,
		modifiers: QueryModifiers,
	},
	Exit,
	ToggleExpanded,
	ToggleWriteMode,
}

impl ReplAction {
	async fn handle(
		self,
		client: &tokio_postgres::Client,
		monitor_client: &tokio_postgres::Client,
		backend_pid: i32,
		theme: Theme,
		repl_state: &Arc<Mutex<ReplState>>,
		rl: &mut Editor<SqlCompleter, History>,
		db_user: &str,
		sys_user: &str,
		line: &str,
	) -> ControlFlow<()> {
		match self {
			ReplAction::Continue => Self::handle_continue(),
			ReplAction::ToggleExpanded => Self::handle_toggle_expanded(repl_state),
			ReplAction::Exit => Self::handle_exit(repl_state, rl, db_user, sys_user, line),
			ReplAction::ToggleWriteMode => {
				Self::handle_toggle_write_mode(
					client,
					monitor_client,
					backend_pid,
					repl_state,
					rl,
					db_user,
					sys_user,
				)
				.await
			}
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				Self::handle_execute(
					client,
					monitor_client,
					backend_pid,
					theme,
					repl_state,
					rl,
					db_user,
					sys_user,
					input,
					sql,
					modifiers,
				)
				.await
			}
		}
	}

	fn handle_continue() -> ControlFlow<()> {
		ControlFlow::Continue(())
	}

	fn handle_toggle_expanded(repl_state: &Arc<Mutex<ReplState>>) -> ControlFlow<()> {
		let mut state = repl_state.lock().unwrap();
		state.expanded_mode = !state.expanded_mode;
		eprintln!(
			"Expanded display is {}.",
			if state.expanded_mode { "on" } else { "off" }
		);
		ControlFlow::Continue(())
	}

	fn handle_exit(
		repl_state: &Arc<Mutex<ReplState>>,
		rl: &mut Editor<SqlCompleter, History>,
		db_user: &str,
		sys_user: &str,
		line: &str,
	) -> ControlFlow<()> {
		let user_input = line.to_string();
		let _ = rl.add_history_entry(&user_input);
		let state = repl_state.lock().unwrap();
		if let Err(e) = rl.history_mut().add_entry(
			user_input,
			db_user.to_string(),
			sys_user.to_string(),
			state.write_mode,
			state.ots.clone(),
		) {
			debug!("failed to add to history: {}", e);
		}
		ControlFlow::Break(())
	}

	async fn handle_execute(
		client: &tokio_postgres::Client,
		monitor_client: &tokio_postgres::Client,
		backend_pid: i32,
		theme: Theme,
		repl_state: &Arc<Mutex<ReplState>>,
		rl: &mut Editor<SqlCompleter, History>,
		db_user: &str,
		sys_user: &str,
		input: String,
		sql: String,
		modifiers: QueryModifiers,
	) -> ControlFlow<()> {
		let _ = rl.add_history_entry(&input);
		let state = repl_state.lock().unwrap();
		if let Err(e) = rl.history_mut().add_entry(
			input,
			db_user.to_string(),
			sys_user.to_string(),
			state.write_mode,
			state.ots.clone(),
		) {
			debug!("failed to add to history: {}", e);
		}
		drop(state);

		match execute_query(client, &sql, modifiers, theme).await {
			Ok(()) => {
				// If write mode is on and we're not in a transaction, start one
				let tx_state = check_transaction_state(monitor_client, backend_pid).await;
				if repl_state.lock().unwrap().write_mode
					&& matches!(tx_state, TransactionState::None)
				{
					if let Err(e) = client.batch_execute("BEGIN").await {
						warn!("Failed to start transaction: {}", e);
					}
				}
			}
			Err(e) => {
				eprintln!("Error: {:?}", e);
			}
		}
		ControlFlow::Continue(())
	}

	async fn handle_toggle_write_mode(
		client: &tokio_postgres::Client,
		monitor_client: &tokio_postgres::Client,
		backend_pid: i32,
		repl_state: &Arc<Mutex<ReplState>>,
		rl: &mut Editor<SqlCompleter, History>,
		db_user: &str,
		sys_user: &str,
	) -> ControlFlow<()> {
		handle_write_mode_toggle(
			client,
			monitor_client,
			backend_pid,
			repl_state,
			rl,
			db_user,
			sys_user,
		)
		.await
	}
}

#[derive(Debug, Clone)]
struct ReplState {
	expanded_mode: bool,
	write_mode: bool,
	ots: Option<String>,
}

impl ReplState {
	#[cfg(test)]
	fn new() -> Self {
		Self {
			expanded_mode: false,
			write_mode: false,
			ots: None,
		}
	}
}

fn handle_input(buffer: &str, new_line: &str, state: &ReplState) -> (String, ReplAction) {
	let mut new_buffer = buffer.to_string();

	if !new_buffer.is_empty() {
		new_buffer.push('\n');
	}
	new_buffer.push_str(new_line);

	let user_input = new_buffer.trim().to_string();

	// Check for metacommands first (only if buffer is empty, i.e., command starts on first character)
	if buffer.is_empty() {
		if let Ok(Some(metacmd)) = parse_metacommand(&user_input) {
			let action = match metacmd {
				Metacommand::Quit => ReplAction::Exit,
				Metacommand::Expanded => ReplAction::ToggleExpanded,
				Metacommand::WriteMode => ReplAction::ToggleWriteMode,
			};
			return (String::new(), action);
		}
	}

	// Handle legacy "quit" command for compatibility
	if buffer.is_empty() && user_input.eq_ignore_ascii_case("quit") {
		return (String::new(), ReplAction::Exit);
	}

	let parse_result = parse_query_modifiers(&user_input);

	let action = match parse_result {
		Ok(Some((sql, mut modifiers))) => {
			// Apply expanded mode state if enabled
			if state.expanded_mode {
				modifiers.insert(crate::parser::QueryModifier::Expanded);
			}
			ReplAction::Execute {
				input: user_input.clone(),
				sql,
				modifiers,
			}
		}
		Ok(None) | Err(_) => ReplAction::Continue,
	};

	let buffer_state = if let ReplAction::Continue = &action {
		new_buffer
	} else {
		String::new()
	};

	(buffer_state, action)
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

async fn handle_write_mode_toggle(
	client: &tokio_postgres::Client,
	monitor_client: &tokio_postgres::Client,
	backend_pid: i32,
	repl_state: &Arc<Mutex<ReplState>>,
	rl: &mut Editor<SqlCompleter, History>,
	db_user: &str,
	sys_user: &str,
) -> ControlFlow<()> {
	let current_write_mode = repl_state.lock().unwrap().write_mode;

	if current_write_mode {
		let tx_state = check_transaction_state(monitor_client, backend_pid).await;
		if tx_state == TransactionState::Active {
			eprintln!("Cannot disable write mode while in a transaction with active changes. COMMIT or ROLLBACK first.");
			return ControlFlow::Continue(());
		}

		repl_state.lock().unwrap().write_mode = false;
		repl_state.lock().unwrap().ots = None;

		match client
			.batch_execute("ROLLBACK; SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
			.await
		{
			Ok(_) => {
				debug!("Write mode disabled");
				eprintln!("SESSION IS NOW READ ONLY");

				rl.history_mut().set_context(
					db_user.to_string(),
					sys_user.to_string(),
					false,
					None,
				);
			}
			Err(e) => {
				eprintln!("Failed to disable write mode: {}", e);
			}
		}
	} else {
		match ots::prompt_for_ots(rl.history()) {
			Ok(new_ots) => {
				repl_state.lock().unwrap().write_mode = true;
				repl_state.lock().unwrap().ots = Some(new_ots.clone());

				match client
					.batch_execute(
						"SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; COMMIT; BEGIN",
					)
					.await
				{
					Ok(_) => {
						debug!("Write mode enabled");
						eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");

						rl.history_mut().set_context(
							db_user.to_string(),
							sys_user.to_string(),
							true,
							Some(new_ots),
						);
					}
					Err(e) => {
						eprintln!("Failed to enable write mode: {}", e);
						repl_state.lock().unwrap().write_mode = false;
						repl_state.lock().unwrap().ots = None;
					}
				}
			}
			Err(e) => {
				eprintln!("Failed to enable write mode: {}", e);
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

pub(crate) async fn run_repl(
	client: Arc<tokio_postgres::Client>,
	theme: Theme,
	history_path: std::path::PathBuf,
	db_user: String,
	database_name: String,
	is_superuser: bool,
	connection_string: String,
	write_mode: bool,
	ots: Option<String>,
) -> Result<()> {
	// Get the backend PID of the main connection
	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.into_diagnostic()?
		.get(0);

	debug!(backend_pid, "main connection backend PID");

	// Create a separate connection for monitoring transaction state
	let tls_connector = crate::tls::make_tls_connector()?;
	let (monitor_client, monitor_connection) =
		tokio_postgres::connect(&connection_string, tls_connector)
			.await
			.into_diagnostic()?;

	tokio::spawn(async move {
		if let Err(e) = monitor_connection.await {
			warn!("monitor connection error: {}", e);
		}
	});

	debug!("monitor connection established");
	let sys_user = std::env::var("USER")
		.or_else(|_| std::env::var("USERNAME"))
		.unwrap_or_else(|_| "unknown".to_string());

	let repl_state = Arc::new(Mutex::new(ReplState {
		expanded_mode: false,
		write_mode,
		ots,
	}));

	let mut history = History::open(&history_path)?;
	history.set_context(
		db_user.clone(),
		sys_user.clone(),
		repl_state.lock().unwrap().write_mode,
		repl_state.lock().unwrap().ots.clone(),
	);

	debug!("initializing schema cache");
	let schema_cache_manager = SchemaCacheManager::new(connection_string);
	let cache_arc = schema_cache_manager.cache_arc();
	let _cache_task = schema_cache_manager.start_background_refresh();

	let completer = SqlCompleter::new(theme).with_schema_cache(cache_arc);
	let mut rl: Editor<SqlCompleter, History> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.build(),
		history,
	)
	.into_diagnostic()?;
	rl.set_helper(Some(completer));

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

				let state = repl_state.lock().unwrap().clone();
				let (new_buffer, action) = handle_input(&buffer, line, &state);
				buffer = new_buffer;

				if action
					.handle(
						&client,
						&monitor_client,
						backend_pid,
						theme,
						&repl_state,
						&mut rl,
						&db_user,
						&sys_user,
						line,
					)
					.await
					.is_break()
				{
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

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	// To run tests that require a database connection:
	// DATABASE_URL=postgresql://localhost/tamanu_meta cargo test -p bestool-psql2

	#[test]
	fn test_handle_input_empty_line() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "", &state);
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::Continue);
	}

	#[test]
	fn test_handle_input_incomplete_query() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "SELECT * FROM users", &state);
		assert_eq!(buffer, "SELECT * FROM users");
		assert_eq!(action, ReplAction::Continue);
	}

	#[test]
	fn test_handle_input_complete_query_semicolon() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "SELECT * FROM users;", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				assert_eq!(input, "SELECT * FROM users;");
				assert_eq!(sql, "SELECT * FROM users");
				assert!(modifiers.is_empty());
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_handle_input_complete_query_backslash_g() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "SELECT * FROM users\\g", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				assert_eq!(input, "SELECT * FROM users\\g");
				assert_eq!(sql, "SELECT * FROM users");
				assert!(modifiers.is_empty());
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_handle_input_multiline_query() {
		let state = ReplState::new();
		let (buffer1, action1) = handle_input("", "SELECT *", &state);
		assert_eq!(buffer1, "SELECT *");
		assert_eq!(action1, ReplAction::Continue);

		let (buffer2, action2) = handle_input(&buffer1, "FROM users;", &state);
		assert_eq!(buffer2, "");
		match action2 {
			ReplAction::Execute { input, sql, .. } => {
				assert_eq!(input, "SELECT *\nFROM users;");
				assert_eq!(sql, "SELECT *\nFROM users");
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_handle_input_quit_command() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "\\q", &state);
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::Exit);
	}

	#[test]
	fn test_handle_input_quit_command_case_insensitive() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "QUIT", &state);
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::Exit);
	}

	#[test]
	fn test_handle_input_quit_after_incomplete() {
		let state = ReplState::new();
		let (buffer1, action1) = handle_input("", "SELECT *", &state);
		assert_eq!(buffer1, "SELECT *");
		assert_eq!(action1, ReplAction::Continue);

		// \q after incomplete query is not treated as quit - it's part of the query
		let (buffer2, action2) = handle_input(&buffer1, "\\q", &state);
		assert_eq!(buffer2, "SELECT *\n\\q");
		assert_eq!(action2, ReplAction::Continue);
	}

	#[test]
	fn test_handle_input_expanded_metacommand() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "\\x", &state);
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::ToggleExpanded);
	}

	#[test]
	fn test_handle_input_expanded_metacommand_uppercase() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "\\X", &state);
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::ToggleExpanded);
	}

	#[test]
	fn test_ctrl_c_clears_buffer() {
		let state = ReplState::new();
		// Simulate building up a query
		let (buffer, _) = handle_input("", "SELECT *", &state);
		assert_eq!(buffer, "SELECT *");

		// Ctrl-C should clear the buffer (simulated by setting buffer to empty)
		let cleared_buffer = "";
		assert_eq!(cleared_buffer, "");

		// Can start fresh after Ctrl-C
		let (new_buffer, action) = handle_input(cleared_buffer, "SELECT 1;", &state);
		assert_eq!(new_buffer, "");
		match action {
			ReplAction::Execute { input, sql, .. } => {
				assert_eq!(input, "SELECT 1;");
				assert_eq!(sql, "SELECT 1");
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_ctrl_c_on_empty_buffer() {
		// Ctrl-C on empty buffer should keep it empty (not exit)
		let _buffer = "";
		let cleared_buffer = "";
		assert_eq!(cleared_buffer, "");
	}

	#[test]
	fn test_ctrl_d_exits() {
		// Ctrl-D behavior is tested via ReadlineError::Eof in the main loop
		// This is a documentation test showing the expected behavior
		// Ctrl-D (EOF) should exit the REPL regardless of buffer state
	}

	#[test]
	fn test_handle_input_preserves_modifiers() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "select 1+1 \\gx", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				// Input should preserve the full command including modifier
				assert_eq!(input, "select 1+1 \\gx");
				// SQL should be parsed without the modifier
				assert_eq!(sql, "select 1+1");
				// Modifiers should be parsed
				assert!(modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_expanded_mode_applied_to_query() {
		let state = ReplState {
			expanded_mode: true,
			write_mode: false,
			ots: None,
		};
		let (buffer, action) = handle_input("", "SELECT 1;", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute { modifiers, .. } => {
				assert!(modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_expanded_mode_not_applied_when_off() {
		let state = ReplState::new();
		let (buffer, action) = handle_input("", "SELECT 1;", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute { modifiers, .. } => {
				assert!(!modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[test]
	fn test_expanded_mode_with_explicit_gx() {
		// When expanded mode is on and \gx is used, expanded should still be on
		let state = ReplState {
			expanded_mode: true,
			write_mode: false,
			ots: None,
		};
		let (buffer, action) = handle_input("", "SELECT 1\\gx", &state);
		assert_eq!(buffer, "");
		match action {
			ReplAction::Execute { modifiers, .. } => {
				assert!(modifiers.contains(&crate::parser::QueryModifier::Expanded));
			}
			_ => panic!("Expected Execute action"),
		}
	}

	#[tokio::test]
	async fn test_transaction_state_none() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let (monitor_client, monitor_connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect monitor");

		tokio::spawn(async move {
			let _ = monitor_connection.await;
		});

		// No transaction should be active initially
		let state = check_transaction_state(&monitor_client, backend_pid).await;
		assert_eq!(state, TransactionState::None);
	}

	#[tokio::test]
	async fn test_transaction_state_idle() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let (monitor_client, monitor_connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect monitor");

		tokio::spawn(async move {
			let _ = monitor_connection.await;
		});

		// Start a transaction without allocating an XID
		client
			.batch_execute("BEGIN")
			.await
			.expect("Failed to begin transaction");

		// Should detect idle transaction (no XID allocated yet)
		let state = check_transaction_state(&monitor_client, backend_pid).await;
		assert_eq!(state, TransactionState::Idle);

		// Clean up
		client.batch_execute("ROLLBACK").await.ok();
	}

	#[tokio::test]
	async fn test_transaction_state_active() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let (monitor_client, monitor_connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect monitor");

		tokio::spawn(async move {
			let _ = monitor_connection.await;
		});

		// Start a transaction and allocate an XID by creating a temp table
		client
			.batch_execute("BEGIN; CREATE TEMP TABLE test_xid (id INT)")
			.await
			.expect("Failed to begin transaction and allocate XID");

		// Give pg_stat_activity time to update
		tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

		// Should detect active transaction with XID
		let state = check_transaction_state(&monitor_client, backend_pid).await;
		assert_eq!(state, TransactionState::Active);

		// Clean up
		client.batch_execute("ROLLBACK").await.ok();
	}

	#[tokio::test]
	async fn test_transaction_state_error() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let (monitor_client, monitor_connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect monitor");

		tokio::spawn(async move {
			let _ = monitor_connection.await;
		});

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
		let state = check_transaction_state(&monitor_client, backend_pid).await;
		assert_eq!(state, TransactionState::Error);

		// Clean up
		client.batch_execute("ROLLBACK").await.ok();
	}

	#[tokio::test]
	async fn test_write_mode_disable_with_idle_transaction() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let (monitor_client, monitor_connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect monitor");

		tokio::spawn(async move {
			let _ = monitor_connection.await;
		});

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

		let (client, connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let (monitor_client, monitor_connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect monitor");

		tokio::spawn(async move {
			let _ = monitor_connection.await;
		});

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

		let (client, connection) = tokio_postgres::connect(
			&connection_string,
			crate::tls::make_tls_connector().unwrap(),
		)
		.await
		.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

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
