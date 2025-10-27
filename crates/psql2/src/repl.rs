use crate::helper::SqlHelper;
use crate::highlighter::Theme;
use crate::history::History;
use crate::parser::{parse_query_modifiers, QueryModifiers};
use crate::query::execute_query;
use miette::{IntoDiagnostic, Result};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use tracing::debug;

pub(crate) async fn run_repl(
	client: tokio_postgres::Client,
	theme: Theme,
	history_path: std::path::PathBuf,
	db_user: String,
	database_name: String,
	is_superuser: bool,
) -> Result<()> {
	let sys_user = std::env::var("USER")
		.or_else(|_| std::env::var("USERNAME"))
		.unwrap_or_else(|_| "unknown".to_string());

	let mut history = History::open(&history_path)?;
	history.set_context(db_user.clone(), sys_user.clone(), false, None);

	let helper = SqlHelper::new(theme);
	let mut rl: Editor<SqlHelper, History> = Editor::with_history(
		rustyline::Config::builder()
			.auto_add_history(false)
			.enable_signals(false)
			.build(),
		history,
	)
	.into_diagnostic()?;
	rl.set_helper(Some(helper));

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

				if !buffer.is_empty() {
					buffer.push('\n');
				}
				buffer.push_str(line);

				let user_input = buffer.trim().to_string();

				// Parse the query to see if it's ready to execute
				let is_quit = user_input.eq_ignore_ascii_case("\\q")
					|| user_input.eq_ignore_ascii_case("quit");
				let parse_result = if is_quit {
					Ok(Some((user_input.clone(), QueryModifiers::new())))
				} else {
					parse_query_modifiers(&user_input)
				};

				let should_execute = parse_result
					.as_ref()
					.ok()
					.and_then(|r| r.as_ref())
					.is_some();

				if should_execute {
					buffer.clear();

					// Always add to history
					let _ = rl.add_history_entry(&user_input);
					if let Err(e) = rl.history_mut().add_entry(
						user_input.clone(),
						db_user.clone(),
						sys_user.clone(),
						false,
						None,
					) {
						debug!("failed to add to history: {}", e);
					}

					// Handle quit
					if is_quit {
						break;
					}

					// Execute query
					match parse_result {
						Ok(Some((sql_to_execute, modifiers))) => {
							match execute_query(&client, &sql_to_execute, modifiers).await {
								Ok(()) => {}
								Err(e) => {
									eprintln!("Error: {:?}", e);
								}
							}
						}
						Ok(None) => {
							// Should not happen since should_execute was true
						}
						Err(e) => {
							eprintln!("Parse error: {:?}", e);
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
