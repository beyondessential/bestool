use crate::helper::SqlHelper;
use crate::highlighter::Theme;
use crate::history::History;
use crate::parser::parse_query_modifiers;
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

				if buffer.is_empty()
					&& (line.eq_ignore_ascii_case("\\q") || line.eq_ignore_ascii_case("quit"))
				{
					break;
				}

				if !buffer.is_empty() {
					buffer.push('\n');
				}
				buffer.push_str(line);

				let user_input = buffer.trim().to_string();
				let (_, test_mods) = parse_query_modifiers(&user_input);
				let has_metacommand = test_mods.expanded
					|| test_mods.json
					|| test_mods.varset
					|| user_input.trim_end().to_lowercase().ends_with("\\g");
				let should_execute = user_input.ends_with(';')
					|| has_metacommand
					|| user_input.eq_ignore_ascii_case("\\q")
					|| user_input.eq_ignore_ascii_case("quit");

				if should_execute {
					buffer.clear();

					if user_input.eq_ignore_ascii_case("\\q")
						|| user_input.eq_ignore_ascii_case("quit")
					{
						break;
					}

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

					let (sql_to_execute, modifiers) = parse_query_modifiers(&user_input);

					match execute_query(&client, &sql_to_execute, modifiers).await {
						Ok(()) => {}
						Err(e) => {
							eprintln!("Error: {:?}", e);
						}
					}
				}
			}
			Err(ReadlineError::Interrupted) => {
				debug!("CTRL-C");
				if !buffer.is_empty() {
					buffer.clear();
					eprintln!("\nQuery buffer cleared");
				} else {
					break;
				}
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
