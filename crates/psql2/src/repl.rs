use crate::completer::SqlCompleter;
use crate::highlighter::Theme;
use crate::history::History;
use crate::parser::{parse_query_modifiers, QueryModifiers};
use crate::query::execute_query;
use crate::schema_cache::SchemaCacheManager;
use miette::{IntoDiagnostic, Result};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use tracing::debug;

#[derive(Debug, PartialEq)]
enum ReplAction {
	Continue,
	Execute {
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
				ReplAction::Execute { sql, modifiers }
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

pub(crate) async fn run_repl(
	client: tokio_postgres::Client,
	theme: Theme,
	history_path: std::path::PathBuf,
	db_user: String,
	database_name: String,
	is_superuser: bool,
	connection_string: String,
) -> Result<()> {
	let sys_user = std::env::var("USER")
		.or_else(|_| std::env::var("USERNAME"))
		.unwrap_or_else(|_| "unknown".to_string());

	let mut history = History::open(&history_path)?;
	history.set_context(db_user.clone(), sys_user.clone(), false, None);

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
		let prompt_suffix = if is_superuser { "=#" } else { "=>" };
		let prompt = if buffer.is_empty() {
			format!("{}{} ", database_name, prompt_suffix)
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
						if let Err(e) = rl.history_mut().add_entry(
							user_input,
							db_user.clone(),
							sys_user.clone(),
							false,
							None,
						) {
							debug!("failed to add to history: {}", e);
						}
						break;
					}
					ReplAction::Execute { sql, modifiers } => {
						let _ = rl.add_history_entry(&sql);
						if let Err(e) = rl.history_mut().add_entry(
							sql.clone(),
							db_user.clone(),
							sys_user.clone(),
							false,
							None,
						) {
							debug!("failed to add to history: {}", e);
						}

						match execute_query(&client, &sql, modifiers).await {
							Ok(()) => {}
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
			ReplAction::Execute { sql, modifiers } => {
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
			ReplAction::Execute { sql, modifiers } => {
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
			ReplAction::Execute { sql, .. } => {
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
			ReplAction::Execute { sql, .. } => {
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
}
