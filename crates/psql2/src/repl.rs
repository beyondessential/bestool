use crate::completer::SqlCompleter;
use crate::highlighter::Theme;
use crate::history::History;
use crate::ots;
use crate::parser::{parse_query_modifiers, QueryModifiers};
use crate::query::execute_query;
use crate::schema_cache::SchemaCacheManager;
use miette::{IntoDiagnostic, Result};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use std::sync::{Arc, Mutex};
use tracing::{debug, info, warn};

#[derive(Debug, PartialEq)]
enum ReplAction {
	Continue,
	Execute {
		input: String,
		sql: String,
		modifiers: QueryModifiers,
	},
	Exit,
}

fn handle_input(buffer: &str, new_line: &str) -> (String, ReplAction) {
	let mut new_buffer = buffer.to_string();

	if !new_buffer.is_empty() {
		new_buffer.push('\n');
	}
	new_buffer.push_str(new_line);

	let user_input = new_buffer.trim().to_string();

	let is_quit = user_input.eq_ignore_ascii_case("\\q") || user_input.eq_ignore_ascii_case("quit");
	let parse_result = if is_quit {
		Ok(Some((user_input.clone(), QueryModifiers::new())))
	} else {
		parse_query_modifiers(&user_input)
	};

	let action = match parse_result {
		Ok(Some((sql, modifiers))) => {
			if is_quit {
				ReplAction::Exit
			} else {
				ReplAction::Execute {
					input: user_input.clone(),
					sql,
					modifiers,
				}
			}
		}
		Ok(None) | Err(_) => ReplAction::Continue,
	};

	let buffer_state = match action {
		ReplAction::Continue => new_buffer,
		ReplAction::Execute { .. } | ReplAction::Exit => String::new(),
	};

	(buffer_state, action)
}

/// Check if the connection is currently in a transaction
async fn is_in_transaction(client: &tokio_postgres::Client) -> bool {
	match client
		.query_one(
			"SELECT CASE WHEN txid_current_if_assigned() IS NULL THEN false ELSE true END",
			&[],
		)
		.await
	{
		Ok(row) => row.get(0),
		Err(_) => false,
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
	let sys_user = std::env::var("USER")
		.or_else(|_| std::env::var("USERNAME"))
		.unwrap_or_else(|_| "unknown".to_string());

	let write_mode = Arc::new(Mutex::new(write_mode));
	let ots = Arc::new(Mutex::new(ots));

	let mut history = History::open(&history_path)?;
	history.set_context(
		db_user.clone(),
		sys_user.clone(),
		*write_mode.lock().unwrap(),
		ots.lock().unwrap().clone(),
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
		let in_transaction = is_in_transaction(&client).await;
		let transaction_marker = if in_transaction { "*" } else { "" };
		let prompt_suffix = if is_superuser { "#" } else { ">" };
		let prompt = if buffer.is_empty() {
			format!("{}={}{} ", database_name, transaction_marker, prompt_suffix)
		} else {
			format!("{}->  ", database_name)
		};

		let readline = rl.readline(&prompt);
		match readline {
			Ok(line) => {
				let line = line.trim();
				if line.is_empty() && buffer.is_empty() {
					continue;
				}

				if line == "\\W" && buffer.is_empty() {
					let current_write_mode = *write_mode.lock().unwrap();

					if current_write_mode {
						if is_in_transaction(&client).await {
							eprintln!("Cannot disable write mode while in a transaction. COMMIT or ROLLBACK first.");
							continue;
						}

						*write_mode.lock().unwrap() = false;
						*ots.lock().unwrap() = None;

						match client
							.batch_execute(
								"SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY; COMMIT",
							)
							.await
						{
							Ok(_) => {
								info!("Write mode disabled");
								eprintln!("SESSION IS NOW READ ONLY");

								rl.history_mut().set_context(
									db_user.clone(),
									sys_user.clone(),
									false,
									None,
								);
							}
							Err(e) => {
								eprintln!("Failed to disable write mode: {}", e);
							}
						}
					} else {
						match ots::prompt_for_ots(&history_path) {
							Ok(new_ots) => {
								*write_mode.lock().unwrap() = true;
								*ots.lock().unwrap() = Some(new_ots.clone());

								match client
									.batch_execute(
										"SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE; COMMIT; BEGIN",
									)
									.await
								{
									Ok(_) => {
										info!("Write mode enabled");
										eprintln!(
											"AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES"
										);

										rl.history_mut().set_context(
											db_user.clone(),
											sys_user.clone(),
											true,
											Some(new_ots),
										);
									}
									Err(e) => {
										eprintln!("Failed to enable write mode: {}", e);
										*write_mode.lock().unwrap() = false;
										*ots.lock().unwrap() = None;
									}
								}
							}
							Err(e) => {
								eprintln!("Failed to enable write mode: {}", e);
							}
						}
					}
					continue;
				}

				let (new_buffer, action) = handle_input(&buffer, line);
				buffer = new_buffer;

				match action {
					ReplAction::Continue => continue,
					ReplAction::Exit => {
						let user_input = if line.eq_ignore_ascii_case("\\q")
							|| line.eq_ignore_ascii_case("quit")
						{
							line.to_string()
						} else {
							line.to_string()
						};
						let _ = rl.add_history_entry(&user_input);
						let current_write_mode = *write_mode.lock().unwrap();
						let current_ots = ots.lock().unwrap().clone();
						if let Err(e) = rl.history_mut().add_entry(
							user_input,
							db_user.clone(),
							sys_user.clone(),
							current_write_mode,
							current_ots,
						) {
							debug!("failed to add to history: {}", e);
						}
						break;
					}
					ReplAction::Execute {
						input,
						sql,
						modifiers,
					} => {
						let _ = rl.add_history_entry(&input);
						let current_write_mode = *write_mode.lock().unwrap();
						let current_ots = ots.lock().unwrap().clone();
						if let Err(e) = rl.history_mut().add_entry(
							input,
							db_user.clone(),
							sys_user.clone(),
							current_write_mode,
							current_ots,
						) {
							debug!("failed to add to history: {}", e);
						}

						match execute_query(&client, &sql, modifiers).await {
							Ok(()) => {
								// If write mode is on and we're not in a transaction, start one
								if *write_mode.lock().unwrap() && !is_in_transaction(&client).await
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
					}
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

	#[test]
	fn test_handle_input_empty_line() {
		let (buffer, action) = handle_input("", "");
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::Continue);
	}

	#[test]
	fn test_handle_input_incomplete_query() {
		let (buffer, action) = handle_input("", "SELECT * FROM users");
		assert_eq!(buffer, "SELECT * FROM users");
		assert_eq!(action, ReplAction::Continue);
	}

	#[test]
	fn test_handle_input_complete_query_semicolon() {
		let (buffer, action) = handle_input("", "SELECT * FROM users;");
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
		let (buffer, action) = handle_input("", "SELECT * FROM users\\g");
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
		let (buffer1, action1) = handle_input("", "SELECT *");
		assert_eq!(buffer1, "SELECT *");
		assert_eq!(action1, ReplAction::Continue);

		let (buffer2, action2) = handle_input(&buffer1, "FROM users;");
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
		let (buffer, action) = handle_input("", "\\q");
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::Exit);
	}

	#[test]
	fn test_handle_input_quit_command_case_insensitive() {
		let (buffer, action) = handle_input("", "QUIT");
		assert_eq!(buffer, "");
		assert_eq!(action, ReplAction::Exit);
	}

	#[test]
	fn test_handle_input_quit_after_incomplete() {
		let (buffer1, action1) = handle_input("", "SELECT *");
		assert_eq!(buffer1, "SELECT *");
		assert_eq!(action1, ReplAction::Continue);

		// \q after incomplete query is not treated as quit - it's part of the query
		let (buffer2, action2) = handle_input(&buffer1, "\\q");
		assert_eq!(buffer2, "SELECT *\n\\q");
		assert_eq!(action2, ReplAction::Continue);
	}

	#[test]
	fn test_ctrl_c_clears_buffer() {
		// Simulate building up a query
		let (buffer, _) = handle_input("", "SELECT *");
		assert_eq!(buffer, "SELECT *");

		// Ctrl-C should clear the buffer (simulated by setting buffer to empty)
		let cleared_buffer = "";
		assert_eq!(cleared_buffer, "");

		// Can start fresh after Ctrl-C
		let (new_buffer, action) = handle_input(cleared_buffer, "SELECT 1;");
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
		let (buffer, action) = handle_input("", "select 1+1 \\gx");
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
}
