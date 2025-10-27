use miette::{IntoDiagnostic, Result};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use thiserror::Error;
use tokio_postgres::NoTls;
use tracing::{debug, info};

pub mod helper;
pub mod highlighter;

use helper::SqlHelper;
use highlighter::Theme;

#[derive(Debug, Error)]
pub enum PsqlError {
	#[error("database connection failed")]
	ConnectionFailed,
	#[error("query execution failed")]
	QueryFailed,
}

/// Configuration for the psql2 client
#[derive(Debug, Clone)]
pub struct PsqlConfig {
	/// Database connection string
	pub connection_string: String,

	/// Database user for tracking
	pub user: Option<String>,

	/// Syntax highlighting theme
	pub theme: Theme,
}

/// Run the psql2 client
pub async fn run(config: PsqlConfig) -> Result<()> {
	let theme = config.theme;
	debug!("connecting to database");
	let (client, connection) = tokio_postgres::connect(&config.connection_string, NoTls)
		.await
		.into_diagnostic()?;

	tokio::spawn(async move {
		if let Err(e) = connection.await {
			eprintln!("connection error: {}", e);
		}
	});

	info!("connected to database");

	debug!("executing version query");
	let rows = client
		.query("SELECT version();", &[])
		.await
		.into_diagnostic()?;

	if let Some(row) = rows.first() {
		let version: String = row.get(0);
		println!("{}", version);
	}

	run_repl(client, theme).await?;

	Ok(())
}

async fn run_repl(client: tokio_postgres::Client, theme: Theme) -> Result<()> {
	let helper = SqlHelper::new(theme);
	let mut rl = Editor::new().into_diagnostic()?;
	rl.set_helper(Some(helper));

	loop {
		let readline = rl.readline("psql2> ");
		match readline {
			Ok(line) => {
				let line = line.trim();
				if line.is_empty() {
					continue;
				}

				let _ = rl.add_history_entry(line);

				if line.eq_ignore_ascii_case("\\q") || line.eq_ignore_ascii_case("quit") {
					break;
				}

				match execute_query(&client, line).await {
					Ok(()) => {}
					Err(e) => {
						eprintln!("Error: {}", e);
					}
				}
			}
			Err(ReadlineError::Interrupted) => {
				debug!("CTRL-C");
				break;
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

async fn execute_query(client: &tokio_postgres::Client, sql: &str) -> Result<()> {
	debug!("executing query: {}", sql);

	let rows = client.query(sql, &[]).await.into_diagnostic()?;

	if rows.is_empty() {
		println!("(no rows)");
		return Ok(());
	}

	if let Some(first_row) = rows.first() {
		let columns = first_row.columns();
		for (i, column) in columns.iter().enumerate() {
			if i > 0 {
				print!(" | ");
			}
			print!("{}", column.name());
		}
		println!();

		for _i in 0..columns.len() {
			print!("----------");
		}
		println!();

		for row in &rows {
			for (i, _column) in columns.iter().enumerate() {
				if i > 0 {
					print!(" | ");
				}
				let value: Option<String> = row.try_get(i).ok();
				print!("{}", value.unwrap_or_else(|| "NULL".to_string()));
			}
			println!();
		}
	}

	println!(
		"\n({} row{})",
		rows.len(),
		if rows.len() == 1 { "" } else { "s" }
	);

	Ok(())
}
