use crate::completer::SqlCompleter;
use crate::config::PsqlConfig;
use crate::highlighter::Theme;
use crate::history::History;
use crate::input::{handle_input, ReplAction};
use crate::ots;
use crate::query::execute_query;
use crate::schema_cache::SchemaCacheManager;
use miette::{bail, IntoDiagnostic, Result};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, instrument, warn};

impl ReplAction {
	pub(crate) async fn handle(
		self,
		client: &tokio_postgres::Client,
		monitor_client: &tokio_postgres::Client,
		backend_pid: i32,
		theme: Theme,
		repl_state: &Arc<Mutex<ReplState>>,
		rl: &mut Editor<SqlCompleter, History>,
		line: &str,
	) -> ControlFlow<()> {
		match self {
			ReplAction::Continue => ControlFlow::Continue(()),
			ReplAction::ToggleExpanded => handle_toggle_expanded(repl_state),
			ReplAction::Exit => handle_exit(repl_state, rl, line),
			ReplAction::ToggleWriteMode => {
				handle_write_mode_toggle(client, monitor_client, backend_pid, repl_state, rl).await
			}
			ReplAction::Execute {
				input,
				sql,
				modifiers,
			} => {
				handle_execute(
					client,
					monitor_client,
					backend_pid,
					theme,
					repl_state,
					rl,
					input,
					sql,
					modifiers,
				)
				.await
			}
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
}

impl ReplState {
	pub fn new() -> Self {
		Self {
			db_user: "testuser".to_string(),
			sys_user: "localuser".to_string(),
			expanded_mode: false,
			write_mode: false,
			ots: None,
		}
	}
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
	line: &str,
) -> ControlFlow<()> {
	{
		let history = rl.history_mut();
		history.set_repl_state(&repl_state.lock().unwrap());
		if let Err(e) = history.add_entry(line.into()) {
			debug!("failed to add to history: {}", e);
		}
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
	input: String,
	sql: String,
	modifiers: crate::parser::QueryModifiers,
) -> ControlFlow<()> {
	{
		let history = rl.history_mut();
		history.set_repl_state(&repl_state.lock().unwrap());
		if let Err(e) = history.add_entry(input) {
			debug!("failed to add to history: {}", e);
		}
	}

	match execute_query(client, &sql, modifiers, theme).await {
		Ok(()) => {
			// If write mode is on and we're not in a transaction, start one
			let tx_state = check_transaction_state(monitor_client, backend_pid).await;
			if repl_state.lock().unwrap().write_mode && matches!(tx_state, TransactionState::None) {
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
) -> ControlFlow<()> {
	let state = { repl_state.lock().unwrap().clone() };

	if state.write_mode {
		let tx_state = check_transaction_state(monitor_client, backend_pid).await;
		if tx_state == TransactionState::Active {
			eprintln!("Cannot disable write mode while in a transaction with active changes. COMMIT or ROLLBACK first.");
			return ControlFlow::Continue(());
		}

		let mut new_state = state.clone();
		new_state.write_mode = false;
		new_state.ots = None;

		match client
			.batch_execute("ROLLBACK; SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")
			.await
		{
			Ok(_) => {
				debug!("Write mode disabled");
				eprintln!("SESSION IS NOW READ ONLY");
				rl.history_mut().set_repl_state(&new_state);
				*repl_state.lock().unwrap() = new_state;
			}
			Err(e) => {
				error!("Failed to disable write mode: {}", e);
			}
		}
	} else {
		match ots::prompt_for_ots(rl.history()) {
			Ok(new_ots) => {
				let mut new_state = state.clone();
				new_state.write_mode = true;
				new_state.ots = Some(new_ots.clone());

				match client
					.batch_execute(
						"SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; COMMIT; BEGIN",
					)
					.await
				{
					Ok(_) => {
						debug!("Write mode enabled");
						eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");
						rl.history_mut().set_repl_state(&new_state);
						*repl_state.lock().unwrap() = new_state;
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
	let theme = config.theme;
	let history_path = if let Some(path) = config.history_path {
		path.clone()
	} else {
		History::default_path()?
	};
	let database_name = config.database_name.clone();
	let db_user = config.user.clone().unwrap_or_else(|| {
		std::env::var("USER")
			.or_else(|_| std::env::var("USERNAME"))
			.unwrap_or_else(|_| "unknown".to_string())
	});

	debug!("connecting to database");
	let tls_connector = crate::tls::make_tls_connector()?;
	let (client, connection) = tokio_postgres::connect(&config.connection_string, tls_connector)
		.await
		.into_diagnostic()?;

	tokio::spawn(async move {
		if let Err(e) = connection.await {
			eprintln!("connection error: {}", e);
		}
	});

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
		(database_name, false)
	};

	let client = Arc::new(client);
	let connection_string = config.connection_string;
	let write_mode = config.write;

	// Get the backend PID of the main connection
	let backend_pid: i32 = client
		.query_one("SELECT pg_backend_pid()", &[])
		.await
		.into_diagnostic()?
		.get(0);
	debug!(pid=%backend_pid, "main connection backend PID");

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

	let repl_state = ReplState {
		sys_user,
		db_user,
		expanded_mode: false,

		// write_mode: true (from the CLI) is handled later
		write_mode: false,
		ots: None,
	};

	let history = History::open(&history_path, repl_state.clone())?;
	let repl_state = Arc::new(Mutex::new(repl_state));

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

	// If --write is given on the CLI, toggle write mode as the first action
	// This saves us from handling prompts/history outside of this function
	if write_mode {
		if ReplAction::ToggleWriteMode
			.handle(
				&client,
				&monitor_client,
				backend_pid,
				theme,
				&repl_state,
				&mut rl,
				"",
			)
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

				if action
					.handle(
						&client,
						&monitor_client,
						backend_pid,
						theme,
						&repl_state,
						&mut rl,
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

	let history_db = rl.history_mut().db.clone();
	drop(rl);

	let history = History {
		db: history_db,
		timestamps: Vec::new(),
		repl_state: ReplState::new(),
	};
	history.compact()?;

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	// To run tests that require a database connection:
	// DATABASE_URL=postgresql://localhost/tamanu_meta cargo test -p bestool-psql2

	#[test]
	fn test_psql_config_creation() {
		let config = PsqlConfig {
			connection_string: "postgresql://localhost/test".to_string(),
			user: Some("testuser".to_string()),
			theme: Theme::Dark,
			history_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
			database_name: "test".to_string(),
			write: false,
		};

		assert_eq!(config.connection_string, "postgresql://localhost/test");
		assert_eq!(config.user, Some("testuser".to_string()));
		assert_eq!(config.database_name, "test");
	}

	#[test]
	fn test_psql_config_no_user() {
		let config = PsqlConfig {
			connection_string: "postgresql://localhost/test".to_string(),
			user: None,
			theme: Theme::Dark,
			history_path: Some(std::path::PathBuf::from("/tmp/history.redb")),
			database_name: "test".to_string(),
			write: false,
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

	#[tokio::test]
	async fn test_text_cast_for_record_types() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) =
			tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
				.await
				.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

		let result = crate::query::execute_query(
			&client,
			"SELECT row(1, 'foo', true) as record",
			crate::parser::QueryModifiers::new(),
			crate::highlighter::Theme::Dark,
		)
		.await;

		assert!(result.is_ok());
	}

	#[tokio::test]
	async fn test_array_formatting() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) =
			tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
				.await
				.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

		let result = crate::query::execute_query(
			&client,
			"SELECT ARRAY[1, 2, 3] as numbers",
			crate::parser::QueryModifiers::new(),
			crate::highlighter::Theme::Dark,
		)
		.await;

		assert!(result.is_ok());
	}

	#[tokio::test]
	async fn test_database_info_query() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let (client, connection) =
			tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
				.await
				.expect("Failed to connect to database");

		tokio::spawn(async move {
			let _ = connection.await;
		});

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
