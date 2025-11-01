use std::{io::Write, ops::ControlFlow};

use comfy_table::Table;
use supports_unicode::Stream;

use crate::parser::ResultSubcommand;

use super::ReplContext;

pub(crate) fn handle_result(
	ctx: &mut ReplContext<'_>,
	subcommand: ResultSubcommand,
) -> ControlFlow<()> {
	match subcommand {
		ResultSubcommand::List { limit, detail } => handle_list(ctx, limit, detail),
		ResultSubcommand::Show {
			n: _,
			format: _,
			to: _,
			only: _,
			limit: _,
			offset: _,
		} => {
			eprintln!("\\re show not yet implemented");
			ControlFlow::Continue(())
		}
	}
}

fn handle_list(ctx: &mut ReplContext<'_>, limit: Option<usize>, detail: bool) -> ControlFlow<()> {
	let mut stdout = std::io::stdout();
	handle_list_impl(&mut stdout, ctx, limit, detail)
}

fn handle_list_impl<W: Write>(
	writer: &mut W,
	ctx: &mut ReplContext<'_>,
	limit: Option<usize>,
	detail: bool,
) -> ControlFlow<()> {
	let limit = limit.unwrap_or(20);
	let use_unicode = supports_unicode::on(Stream::Stdout);

	let state = ctx.repl_state.lock().unwrap();
	let store = &state.result_store;

	if store.is_empty() {
		let _ = writeln!(writer, "Nothing yet");
		return ControlFlow::Continue(());
	}

	let _ = writeln!(
		writer,
		"Past query results ({} of {}):\n",
		std::cmp::min(limit, store.len()),
		store.len()
	);

	let mut table = Table::new();
	crate::table::configure(&mut table);

	if detail {
		table.set_header(vec![
			"N", "When", "Took", "Size", "Rows", "Columns", "Query",
		]);
	} else {
		table.set_header(vec!["N", "When", "Took", "Size", "Rows", "Cols"]);
	}

	// List results, newest first
	let results: Vec<_> = store.iter().collect();
	let start_idx = results.len().saturating_sub(limit);

	for (i, result) in results.iter().enumerate().skip(start_idx) {
		let row_count = result.rows.len();
		let column_count = if row_count > 0 {
			result.rows[0].len()
		} else {
			0
		};

		let size_str = format_size(result.estimated_size);
		let datetime_str = format_datetime(&result.timestamp);
		let duration_str = format_duration(&result.duration);

		if detail {
			let columns_str = if row_count > 0 {
				result.rows[0]
					.columns()
					.iter()
					.map(|col| col.name())
					.collect::<Vec<_>>()
					.join(", ")
			} else {
				String::new()
			};

			let ellipsis = if use_unicode { "…" } else { "..." };
			let query_preview = if result.query.len() > 50 {
				format!("{}{}", &result.query[..50], ellipsis)
			} else {
				result.query.clone()
			};

			table.add_row(vec![
				i.to_string(),
				datetime_str,
				duration_str,
				size_str,
				row_count.to_string(),
				columns_str,
				query_preview,
			]);
		} else {
			table.add_row(vec![
				i.to_string(),
				datetime_str,
				duration_str,
				size_str,
				row_count.to_string(),
				column_count.to_string(),
			]);
		}
	}

	crate::table::style_header(&mut table);
	let _ = writeln!(writer, "{table}");

	let _ = writeln!(
		writer,
		"\nTotal memory used: {}",
		format_size(store.total_size())
	);
	let _ = writeln!(writer, "Memory limit: {}\n", format_size(store.max_size()));

	ControlFlow::Continue(())
}

fn format_datetime(timestamp: &jiff::Timestamp) -> String {
	let dt = timestamp.to_zoned(jiff::tz::TimeZone::system());
	dt.strftime("%Y-%m-%d %H:%M:%S").to_string()
}

fn format_duration(duration: &std::time::Duration) -> String {
	let millis = duration.as_secs_f64() * 1000.0;

	if millis < 1.0 {
		format!("{:.2} μs", millis * 1000.0)
	} else if millis < 1000.0 {
		format!("{:.2} ms", millis)
	} else if millis < 60_000.0 {
		format!("{:.2} s", millis / 1000.0)
	} else {
		let seconds = duration.as_secs();
		let minutes = seconds / 60;
		let remaining_seconds = seconds % 60;
		format!("{}:{:02}", minutes, remaining_seconds)
	}
}

fn format_size(bytes: usize) -> String {
	const KB: usize = 1024;
	const MB: usize = KB * 1024;
	const GB: usize = MB * 1024;

	if bytes >= GB {
		format!("{:.2} GB", bytes as f64 / GB as f64)
	} else if bytes >= MB {
		format!("{:.2} MB", bytes as f64 / MB as f64)
	} else if bytes >= KB {
		format!("{:.2} KB", bytes as f64 / KB as f64)
	} else {
		format!("{} B", bytes)
	}
}

#[cfg(test)]
mod tests {
	use std::sync::{Arc, Mutex};

	use super::*;
	use crate::repl::ReplState;

	#[tokio::test]
	async fn test_list_empty_store() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");
		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let repl_state = Arc::new(Mutex::new(ReplState::new()));

		let audit_path = tempfile::NamedTempFile::new()
			.unwrap()
			.into_temp_path()
			.to_path_buf();
		let audit = crate::audit::Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();

		let mut rl: rustyline::Editor<crate::completer::SqlCompleter, crate::audit::Audit> =
			rustyline::Editor::with_history(
				rustyline::Config::builder().auto_add_history(false).build(),
				audit,
			)
			.unwrap();

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		let mut output = Vec::new();
		let result = handle_list_impl(&mut output, &mut ctx, None, false);
		assert_eq!(result, ControlFlow::Continue(()));

		let output_str = String::from_utf8(output).unwrap();
		assert!(output_str.contains("Nothing yet"));
	}

	#[tokio::test]
	async fn test_list_with_results() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");
		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let repl_state = Arc::new(Mutex::new(ReplState::new()));

		// Execute some queries to populate the result store
		let mut stdout = tokio::io::stdout();
		let mut query_ctx = crate::query::QueryContext {
			client: &client,
			modifiers: crate::parser::QueryModifiers::new(),
			theme: crate::theme::Theme::Dark,
			writer: &mut stdout,
			use_colours: true,
			vars: None,
			repl_state: &repl_state,
		};

		crate::query::execute_query("SELECT 1 as num", &mut query_ctx)
			.await
			.expect("Query failed");
		crate::query::execute_query("SELECT 'test' as text, 42 as answer", &mut query_ctx)
			.await
			.expect("Query failed");

		let audit_path = tempfile::NamedTempFile::new()
			.unwrap()
			.into_temp_path()
			.to_path_buf();
		let audit = crate::audit::Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();

		let mut rl: rustyline::Editor<crate::completer::SqlCompleter, crate::audit::Audit> =
			rustyline::Editor::with_history(
				rustyline::Config::builder().auto_add_history(false).build(),
				audit,
			)
			.unwrap();

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		// Test basic list
		let mut output = Vec::new();
		let result = handle_list_impl(&mut output, &mut ctx, None, false);
		assert_eq!(result, ControlFlow::Continue(()));

		let output_str = String::from_utf8(output).unwrap();
		assert!(output_str.contains("Past query results (2 of 2)"));
		assert!(output_str.contains("Total memory used:"));
		assert!(output_str.contains("Memory limit:"));

		// Test detail list
		let mut output = Vec::new();
		let result = handle_list_impl(&mut output, &mut ctx, None, true);
		assert_eq!(result, ControlFlow::Continue(()));

		let output_str = String::from_utf8(output).unwrap();
		assert!(output_str.contains("num"));
		assert!(output_str.contains("text") && output_str.contains("answer"));
		assert!(output_str.contains("SELECT"));
	}

	#[tokio::test]
	async fn test_list_with_limit() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");
		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let repl_state = Arc::new(Mutex::new(ReplState::new()));

		let mut stdout = tokio::io::stdout();
		let mut query_ctx = crate::query::QueryContext {
			client: &client,
			modifiers: crate::parser::QueryModifiers::new(),
			theme: crate::theme::Theme::Dark,
			writer: &mut stdout,
			use_colours: true,
			vars: None,
			repl_state: &repl_state,
		};

		for i in 1..=5 {
			crate::query::execute_query(&format!("SELECT {}", i), &mut query_ctx)
				.await
				.expect("Query failed");
		}

		let audit_path = tempfile::NamedTempFile::new()
			.unwrap()
			.into_temp_path()
			.to_path_buf();
		let audit = crate::audit::Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();

		let mut rl: rustyline::Editor<crate::completer::SqlCompleter, crate::audit::Audit> =
			rustyline::Editor::with_history(
				rustyline::Config::builder().auto_add_history(false).build(),
				audit,
			)
			.unwrap();

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		let mut output = Vec::new();
		let result = handle_list_impl(&mut output, &mut ctx, Some(2), false);
		assert_eq!(result, ControlFlow::Continue(()));

		let output_str = String::from_utf8(output).unwrap();
		assert!(output_str.contains("(2 of 5)"));
	}

	#[tokio::test]
	async fn test_list_query_truncation() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");
		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let repl_state = Arc::new(Mutex::new(ReplState::new()));

		let mut stdout = tokio::io::stdout();
		let mut query_ctx = crate::query::QueryContext {
			client: &client,
			modifiers: crate::parser::QueryModifiers::new(),
			theme: crate::theme::Theme::Dark,
			writer: &mut stdout,
			use_colours: true,
			vars: None,
			repl_state: &repl_state,
		};

		crate::query::execute_query(
			"SELECT 'This is a very long query that should be truncated at fifty characters'",
			&mut query_ctx,
		)
		.await
		.expect("Query failed");

		let audit_path = tempfile::NamedTempFile::new()
			.unwrap()
			.into_temp_path()
			.to_path_buf();
		let audit = crate::audit::Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();

		let mut rl: rustyline::Editor<crate::completer::SqlCompleter, crate::audit::Audit> =
			rustyline::Editor::with_history(
				rustyline::Config::builder().auto_add_history(false).build(),
				audit,
			)
			.unwrap();

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		let mut output = Vec::new();
		let result = handle_list_impl(&mut output, &mut ctx, None, true);
		assert_eq!(result, ControlFlow::Continue(()));

		let output_str = String::from_utf8(output).unwrap();
		// Check that query is truncated (should contain ellipsis)
		assert!(output_str.contains("SELECT"));
		assert!(
			output_str.contains("…") || output_str.contains("..."),
			"Expected ellipsis in output"
		);
	}

	#[tokio::test]
	async fn test_re_list_command() {
		let connection_string =
			std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for this test");

		let pool = crate::pool::create_pool(&connection_string)
			.await
			.expect("Failed to create pool");

		let client = pool.get().await.expect("Failed to get connection");
		let monitor_client = pool.get().await.expect("Failed to get monitor connection");

		let backend_pid: i32 = client
			.query_one("SELECT pg_backend_pid()", &[])
			.await
			.expect("Failed to get backend PID")
			.get(0);

		let repl_state = Arc::new(Mutex::new(ReplState::new()));

		// Execute some queries to populate the result store
		let mut stdout = tokio::io::stdout();
		let mut query_ctx = crate::query::QueryContext {
			client: &client,
			modifiers: crate::parser::QueryModifiers::new(),
			theme: crate::theme::Theme::Dark,
			writer: &mut stdout,
			use_colours: true,
			vars: None,
			repl_state: &repl_state,
		};

		crate::query::execute_query("SELECT 1 as num", &mut query_ctx)
			.await
			.expect("Query failed");
		crate::query::execute_query("SELECT 'test' as text", &mut query_ctx)
			.await
			.expect("Query failed");
		crate::query::execute_query("SELECT 42, 'hello', true", &mut query_ctx)
			.await
			.expect("Query failed");

		// Verify results are stored
		{
			let state = repl_state.lock().unwrap();
			assert_eq!(state.result_store.len(), 3);
		}

		// Create a ReplContext for testing the list command
		let audit_path = tempfile::NamedTempFile::new()
			.unwrap()
			.into_temp_path()
			.to_path_buf();
		let audit = crate::audit::Audit::open(&audit_path, Arc::clone(&repl_state)).unwrap();

		let mut rl: rustyline::Editor<crate::completer::SqlCompleter, crate::audit::Audit> =
			rustyline::Editor::with_history(
				rustyline::Config::builder().auto_add_history(false).build(),
				audit,
			)
			.unwrap();

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
		};

		// Test \re list with no limit (should default to 20)
		let result = crate::repl::result::handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::List {
				limit: None,
				detail: false,
			},
		);
		assert_eq!(result, std::ops::ControlFlow::Continue(()));

		// Test \re list with limit
		let result = crate::repl::result::handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::List {
				limit: Some(2),
				detail: false,
			},
		);
		assert_eq!(result, std::ops::ControlFlow::Continue(()));

		// Test \re list+ (detail mode)
		let result = crate::repl::result::handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::List {
				limit: None,
				detail: true,
			},
		);
		assert_eq!(result, std::ops::ControlFlow::Continue(()));

		// Test \re list when store is empty
		{
			let mut state = repl_state.lock().unwrap();
			state.result_store.clear();
		}

		let result = crate::repl::result::handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::List {
				limit: None,
				detail: false,
			},
		);
		assert_eq!(result, std::ops::ControlFlow::Continue(()));
	}
}
