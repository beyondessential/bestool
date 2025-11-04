use std::{io::Write, ops::ControlFlow};

use comfy_table::Table;
use supports_unicode::Stream;

use crate::parser::{ResultFormat, ResultSubcommand};

use super::ReplContext;

pub(crate) async fn handle_result(
	ctx: &mut ReplContext<'_>,
	subcommand: ResultSubcommand,
) -> ControlFlow<()> {
	match subcommand {
		ResultSubcommand::List { limit, detail } => handle_list(ctx, limit, detail),
		ResultSubcommand::Show {
			n,
			format,
			to,
			cols,
			limit,
			offset,
		} => handle_show(ctx, n, format, to, cols, limit, offset).await,
	}
}

async fn handle_show(
	ctx: &mut ReplContext<'_>,
	n: Option<usize>,
	format: Option<ResultFormat>,
	to: Option<String>,
	cols: Vec<String>,
	limit: Option<usize>,
	offset: Option<usize>,
) -> ControlFlow<()> {
	let mut result = {
		let state = ctx.repl_state.lock().unwrap();
		let result = if let Some(index) = n {
			state.result_store.get(index)
		} else {
			state.result_store.get_last()
		};

		let Some(result) = result else {
			if let Some(index) = n {
				eprintln!("No result at index {}", index);
			} else {
				eprintln!("No results available");
			}
			return ControlFlow::Continue(());
		};

		// Clone the result so we can release the lock
		result.clone()
	};

	// Validate and compute column indices if filtering by column names
	let column_indices = if !cols.is_empty() {
		match compute_column_indices(&result, &cols) {
			ControlFlow::Continue(indices) => Some(indices),
			ControlFlow::Break(()) => return ControlFlow::Continue(()),
		}
	} else {
		None
	};

	// Apply offset
	if let Some(offset_val) = offset {
		if offset_val < result.rows.len() {
			result.rows.drain(..offset_val);
		} else {
			result.rows.clear();
		}
	}

	// Apply limit
	if let Some(limit_val) = limit
		&& limit_val < result.rows.len()
	{
		result.rows.truncate(limit_val);
	}

	// Determine format (default to table)
	let format = format.unwrap_or(ResultFormat::Table);

	// Check if format requires file output
	if format.is_file_only() && to.is_none() {
		let state = ctx.repl_state.lock().unwrap();
		if state.output_file.is_none() {
			eprintln!(
				"Error: {} format can only be written to a file. Use 'to=<path>' to specify output file.",
				match format {
					ResultFormat::Excel => "Excel",
					ResultFormat::Sqlite => "SQLite",
					_ => "This",
				}
			);
			return ControlFlow::Continue(());
		}
	}

	let use_global_output = to.is_none() && {
		let state = ctx.repl_state.lock().unwrap();
		state.output_file.is_some()
	};

	let display_result = if let Some(path) = to {
		display_to_file(ctx, &result, format, &path, column_indices.as_deref()).await
	} else if use_global_output {
		display_to_global_output(ctx, &result, format, column_indices.as_deref()).await
	} else {
		display_to_stdout(ctx, &result, format, column_indices.as_deref()).await
	};

	if let Err(e) = display_result {
		eprintln!("Error displaying result: {}", e);
	}

	ControlFlow::Continue(())
}

async fn display_to_stdout(
	ctx: &mut ReplContext<'_>,
	result: &crate::result_store::StoredResult,
	format: ResultFormat,
	column_indices: Option<&[usize]>,
) -> miette::Result<()> {
	if result.rows.is_empty() {
		println!("(no rows)");
		return Ok(());
	}

	let use_colours = ctx.repl_state.lock().unwrap().use_colours;
	let output =
		format_result_using_display_module(ctx, result, format, use_colours, column_indices)
			.await?;
	print!("{}", output);
	Ok(())
}

async fn display_to_file(
	ctx: &mut ReplContext<'_>,
	result: &crate::result_store::StoredResult,
	format: ResultFormat,
	path: &str,
	column_indices: Option<&[usize]>,
) -> miette::Result<()> {
	use std::io::Write;

	// Check if file already exists
	if std::path::Path::new(path).exists() {
		return Err(miette::miette!("File already exists: {}", path));
	}

	if result.rows.is_empty() {
		let mut file = std::fs::File::create(path)
			.map_err(|e| miette::miette!("Failed to create file '{}': {}", path, e))?;
		writeln!(file, "(no rows)")
			.map_err(|e| miette::miette!("Failed to write to file: {}", e))?;

		// Get absolute path for display
		let display_path =
			std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
		eprintln!("Output written to {}", display_path.display());
		return Ok(());
	}

	// Handle file-only formats directly
	if matches!(format, ResultFormat::Excel | ResultFormat::Sqlite) {
		let first_row = &result.rows[0];
		let columns = first_row.columns();

		// Check for unprintable columns
		let mut unprintable_columns = Vec::new();
		for (i, _column) in columns.iter().enumerate() {
			if !crate::query::column::can_print(first_row, i) {
				unprintable_columns.push(i);
			}
		}

		// Re-query with text casts if needed
		let text_rows = if !unprintable_columns.is_empty() {
			let sql_trimmed = result.query.trim_end_matches(';').trim();
			let text_query =
				crate::query::build_text_cast_query(sql_trimmed, columns, &unprintable_columns);

			ctx.client.query(&text_query, &[]).await.ok()
		} else {
			None
		};

		let mut buffer = Vec::new();
		let display_ctx = crate::query::display::DisplayContext {
			columns,
			rows: &result.rows,
			unprintable_columns: &unprintable_columns,
			text_rows: &text_rows,
			writer: &mut buffer,
			use_colours: false,
			theme: ctx.theme,
			column_indices,
		};

		match format {
			ResultFormat::Excel => {
				crate::query::display::display_excel(&display_ctx, path).await?;
			}
			ResultFormat::Sqlite => {
				crate::query::display::display_sqlite(&display_ctx, path).await?;
			}
			_ => unreachable!(),
		}

		// Get absolute path for display
		let display_path =
			std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
		eprintln!("Output written to {}", display_path.display());
		return Ok(());
	}

	let output =
		format_result_using_display_module(ctx, result, format, false, column_indices).await?;

	let mut file = std::fs::File::create(path)
		.map_err(|e| miette::miette!("Failed to create file '{}': {}", path, e))?;
	file.write_all(output.as_bytes())
		.map_err(|e| miette::miette!("Failed to write to file: {}", e))?;

	// Get absolute path for display
	let display_path =
		std::fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
	eprintln!("Output written to {}", display_path.display());
	Ok(())
}

async fn display_to_global_output(
	ctx: &mut ReplContext<'_>,
	result: &crate::result_store::StoredResult,
	format: ResultFormat,
	column_indices: Option<&[usize]>,
) -> miette::Result<()> {
	use tokio::io::AsyncWriteExt;

	if result.rows.is_empty() {
		if let Some(output_file) = {
			let state = ctx.repl_state.lock().unwrap();
			state.output_file.clone()
		} {
			let mut file = output_file.lock().await;
			file.write_all(b"(no rows)\n")
				.await
				.map_err(|e| miette::miette!("Failed to write to output file: {}", e))?;
			file.flush()
				.await
				.map_err(|e| miette::miette!("Failed to flush output file: {}", e))?;
		}
		return Ok(());
	}

	// File-only formats cannot use global output file
	// because we can't get the path from tokio::fs::File
	let output_path: Option<String> = None;

	// Handle file-only formats
	if matches!(format, ResultFormat::Excel | ResultFormat::Sqlite) {
		if output_path.is_none() {
			return Err(miette::miette!(
				"File-only formats require a file path, but global output file path is not available"
			));
		}
		// For file-only formats with global output, we can't support them properly
		// because we don't have a way to get the file path from tokio::fs::File
		return Err(miette::miette!(
			"File-only formats (Excel, SQLite) are not supported with global output file. Use 'to=<path>' instead."
		));
	}

	// Format without colors for file output
	let output =
		format_result_using_display_module(ctx, result, format, false, column_indices).await?;

	if let Some(output_file) = {
		let state = ctx.repl_state.lock().unwrap();
		state.output_file.clone()
	} {
		let mut file = output_file.lock().await;
		file.write_all(output.as_bytes())
			.await
			.map_err(|e| miette::miette!("Failed to write to output file: {}", e))?;
		file.flush()
			.await
			.map_err(|e| miette::miette!("Failed to flush output file: {}", e))?;
	}

	Ok(())
}

fn compute_column_indices(
	result: &crate::result_store::StoredResult,
	cols: &[String],
) -> ControlFlow<(), Vec<usize>> {
	if result.rows.is_empty() {
		return ControlFlow::Continue(Vec::new());
	}

	// Get column names from the first row
	let columns = result.rows[0].columns();
	let column_names: Vec<String> = columns.iter().map(|c| c.name().to_string()).collect();

	// Find indices of requested columns
	let mut indices = Vec::new();
	for col_name in cols {
		if let Some(idx) = column_names.iter().position(|name| name == col_name) {
			indices.push(idx);
		} else {
			eprintln!("Column '{}' not found in result", col_name);
			return ControlFlow::Break(());
		}
	}

	if indices.is_empty() {
		eprintln!("No valid columns specified");
		return ControlFlow::Break(());
	}

	ControlFlow::Continue(indices)
}

async fn format_result_using_display_module(
	ctx: &mut ReplContext<'_>,
	result: &crate::result_store::StoredResult,
	format: ResultFormat,
	use_colours: bool,
	column_indices: Option<&[usize]>,
) -> miette::Result<String> {
	let first_row = &result.rows[0];
	let columns = first_row.columns();

	// Check for unprintable columns
	let mut unprintable_columns = Vec::new();
	for (i, _column) in columns.iter().enumerate() {
		if !crate::query::column::can_print(first_row, i) {
			unprintable_columns.push(i);
		}
	}

	// Re-query with text casts if needed (for unprintable columns)
	let text_rows = if !unprintable_columns.is_empty() {
		let sql_trimmed = result.query.trim_end_matches(';').trim();
		let text_query =
			crate::query::build_text_cast_query(sql_trimmed, columns, &unprintable_columns);

		ctx.client.query(&text_query, &[]).await.ok()
	} else {
		None
	};

	// Create a buffer to capture output
	let mut buffer = Vec::new();

	// Use the display module's display function
	let mut display_ctx = crate::query::display::DisplayContext {
		columns,
		rows: &result.rows,
		unprintable_columns: &unprintable_columns,
		text_rows: &text_rows,
		writer: &mut buffer,
		use_colours,
		theme: ctx.theme,
		column_indices,
	};

	match format {
		ResultFormat::Table => {
			crate::query::display::display(&mut display_ctx, false, false).await?;
		}
		ResultFormat::Expanded => {
			crate::query::display::display(&mut display_ctx, false, true).await?;
		}
		ResultFormat::Json => {
			crate::query::display::display(&mut display_ctx, true, false).await?;
		}
		ResultFormat::JsonPretty => {
			crate::query::display::display(&mut display_ctx, true, true).await?;
		}
		ResultFormat::Csv => {
			crate::query::display::display_csv(&mut display_ctx).await?;
		}
		ResultFormat::Excel | ResultFormat::Sqlite => {
			return Err(miette::miette!(
				"File-only formats should be handled by display_to_file"
			));
		}
	}

	String::from_utf8(buffer).map_err(|e| miette::miette!("Invalid UTF-8 in output: {}", e))
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

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_last_result() {
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test \re show without n (should show last result)
		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: None,
				to: None,
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_with_index() {
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

		// Execute multiple queries
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

		crate::query::execute_query("SELECT 1 as first", &mut query_ctx)
			.await
			.expect("Query failed");
		crate::query::execute_query("SELECT 2 as second", &mut query_ctx)
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test \re show n=0 (should show first result)
		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: Some(0),
				format: None,
				to: None,
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_with_format() {
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

		crate::query::execute_query("SELECT 'hello' as greeting", &mut query_ctx)
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test different formats
		for format in &[
			crate::parser::ResultFormat::Table,
			crate::parser::ResultFormat::Expanded,
			crate::parser::ResultFormat::Json,
			crate::parser::ResultFormat::JsonPretty,
		] {
			let result = handle_result(
				&mut ctx,
				crate::parser::ResultSubcommand::Show {
					n: None,
					format: Some(format.clone()),
					to: None,
					cols: vec![],
					limit: None,
					offset: None,
				},
			)
			.await;
			assert_eq!(result, ControlFlow::Continue(()));
		}
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_to_file() {
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

		crate::query::execute_query("SELECT 42 as answer", &mut query_ctx)
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test writing to file
		let temp_file = tempfile::NamedTempFile::new().unwrap();
		let file_path = temp_file.path().to_string_lossy().to_string();
		drop(temp_file); // Delete the temp file so the path doesn't exist

		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::Table),
				to: Some(file_path.clone()),
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		// Verify file was created and has content
		let content = std::fs::read_to_string(&file_path).expect("Failed to read output file");
		assert!(!content.is_empty());
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_json_format() {
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

		// Execute a query that returns multiple rows
		crate::query::execute_query(
			"SELECT i as num FROM generate_series(1, 3) i",
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test json format - should output one object per line
		let temp_file_json = tempfile::NamedTempFile::new().unwrap();
		let file_path_json = temp_file_json.path().to_string_lossy().to_string();
		drop(temp_file_json); // Delete the temp file so the path doesn't exist

		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::Json),
				to: Some(file_path_json.clone()),
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		let content_json =
			std::fs::read_to_string(&file_path_json).expect("Failed to read output file");
		let lines: Vec<&str> = content_json.trim().lines().collect();
		// Should have 3 lines, one per row
		assert_eq!(lines.len(), 3);
		// Each line should be a valid JSON object
		for line in &lines {
			assert!(serde_json::from_str::<serde_json::Value>(line).is_ok());
		}

		// Test json-pretty format - should output a single array
		let temp_file_pretty = tempfile::NamedTempFile::new().unwrap();
		let file_path_pretty = temp_file_pretty.path().to_string_lossy().to_string();
		drop(temp_file_pretty); // Delete the temp file so the path doesn't exist

		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::JsonPretty),
				to: Some(file_path_pretty.clone()),
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		let content_pretty =
			std::fs::read_to_string(&file_path_pretty).expect("Failed to read output file");
		// Should parse as a single array
		let parsed: serde_json::Value =
			serde_json::from_str(&content_pretty).expect("Should be valid JSON");
		assert!(parsed.is_array());
		assert_eq!(parsed.as_array().unwrap().len(), 3);
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_csv_format() {
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

		// Execute query to populate the result store
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
			"SELECT 1 as id, 'Alice' as name, 25 as age UNION ALL SELECT 2, 'Bob', 30 UNION ALL SELECT 3, 'Charlie', 35",
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test csv format
		let temp_file_csv = tempfile::NamedTempFile::new().unwrap();
		let file_path_csv = temp_file_csv.path().to_string_lossy().to_string();
		drop(temp_file_csv); // Delete the temp file so the path doesn't exist

		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::Csv),
				to: Some(file_path_csv.clone()),
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		let content_csv =
			std::fs::read_to_string(&file_path_csv).expect("Failed to read output file");
		let lines: Vec<&str> = content_csv.trim().lines().collect();
		// Should have 4 lines: 1 header + 3 data rows
		assert_eq!(lines.len(), 4);
		// First line should be the header
		assert_eq!(lines[0], "id,name,age");
		// Check data rows
		assert_eq!(lines[1], "1,Alice,25");
		assert_eq!(lines[2], "2,Bob,30");
		assert_eq!(lines[3], "3,Charlie,35");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_excel_format() {
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

		// Execute query to populate the result store
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
			"SELECT 1 as id, 'Alice' as name, 25 as age UNION ALL SELECT 2, 'Bob', 30 UNION ALL SELECT 3, 'Charlie', 35",
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test excel format - should require to= parameter
		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::Excel),
				to: None,
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		// Should return Continue (error printed to stderr)
		assert_eq!(result, ControlFlow::Continue(()));

		// Test excel format with file output
		let temp_file_excel = tempfile::NamedTempFile::new().unwrap();
		let file_path_excel = temp_file_excel.path().to_string_lossy().to_string();
		drop(temp_file_excel); // Delete the temp file so the path doesn't exist

		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::Excel),
				to: Some(file_path_excel.clone()),
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		// Verify the Excel file was created
		assert!(std::path::Path::new(&file_path_excel).exists());
		let metadata = std::fs::metadata(&file_path_excel).unwrap();
		assert!(metadata.len() > 0);
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_sqlite_format() {
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

		// Execute query to populate the result store
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
			"SELECT 1 as id, 'Alice' as name, 25 as age UNION ALL SELECT 2, 'Bob', 30 UNION ALL SELECT 3, 'Charlie', 35",
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test sqlite format - should require to= parameter
		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::Sqlite),
				to: None,
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		// Should return Continue (error printed to stderr)
		assert_eq!(result, ControlFlow::Continue(()));

		// Test sqlite format with file output
		let temp_file_sqlite = tempfile::NamedTempFile::new().unwrap();
		let file_path_sqlite = temp_file_sqlite.path().to_string_lossy().to_string();
		drop(temp_file_sqlite); // Delete the temp file so the path doesn't exist

		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::Sqlite),
				to: Some(file_path_sqlite.clone()),
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		// Verify the SQLite database was created and has correct data
		assert!(std::path::Path::new(&file_path_sqlite).exists());
		let verify_db = turso::Builder::new_local(&file_path_sqlite)
			.build()
			.await
			.unwrap();
		let verify_conn = verify_db.connect().unwrap();
		let mut result_rows = verify_conn
			.query("SELECT id, name, age FROM results ORDER BY id", ())
			.await
			.unwrap();

		let row1 = result_rows.next().await.unwrap().unwrap();
		assert_eq!(row1.get_value(0).unwrap().as_text().unwrap(), "1");
		assert_eq!(row1.get_value(1).unwrap().as_text().unwrap(), "Alice");
		assert_eq!(row1.get_value(2).unwrap().as_text().unwrap(), "25");

		let row2 = result_rows.next().await.unwrap().unwrap();
		assert_eq!(row2.get_value(0).unwrap().as_text().unwrap(), "2");
		assert_eq!(row2.get_value(1).unwrap().as_text().unwrap(), "Bob");
		assert_eq!(row2.get_value(2).unwrap().as_text().unwrap(), "30");

		let row3 = result_rows.next().await.unwrap().unwrap();
		assert_eq!(row3.get_value(0).unwrap().as_text().unwrap(), "3");
		assert_eq!(row3.get_value(1).unwrap().as_text().unwrap(), "Charlie");
		assert_eq!(row3.get_value(2).unwrap().as_text().unwrap(), "35");

		assert!(result_rows.next().await.unwrap().is_none());
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_json_highlighting() {
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

		crate::query::execute_query("SELECT 'test' as text, 42 as num", &mut query_ctx)
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// We can't easily capture stdout in tests, but we can verify the function
		// accepts use_colours=true and doesn't error
		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::Json),
				to: None,
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		// When writing to file, colors should not be present
		let temp_file = tempfile::NamedTempFile::new().unwrap();
		let file_path = temp_file.path().to_string_lossy().to_string();
		drop(temp_file); // Delete the temp file so the path doesn't exist

		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::JsonPretty),
				to: Some(file_path.clone()),
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		// Verify file has no ANSI escape codes
		let content = std::fs::read_to_string(&file_path).expect("Failed to read output file");
		assert!(
			!content.contains("\x1b["),
			"File output should not contain color codes"
		);
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_with_global_output_file() {
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

		crate::query::execute_query("SELECT 123 as num", &mut query_ctx)
			.await
			.expect("Query failed");

		// Set up global output file
		let temp_file = tempfile::NamedTempFile::new().unwrap();
		let file_path = temp_file.path().to_path_buf();
		drop(temp_file); // Delete the temp file so the path doesn't exist
		let global_file = tokio::fs::File::create(&file_path).await.unwrap();

		{
			let mut state = repl_state.lock().unwrap();
			state.output_file = Some(Arc::new(tokio::sync::Mutex::new(global_file)));
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test \re show without 'to' parameter - should use global output file
		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::Table),
				to: None,
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		// Close the file handle
		{
			let mut state = repl_state.lock().unwrap();
			state.output_file = None;
		}

		// Verify content was written to global output file
		let content = std::fs::read_to_string(&file_path).expect("Failed to read output file");
		assert!(!content.is_empty());
		assert!(content.contains("123") || content.contains("num"));

		// Test with explicit 'to' parameter - should override global output
		let temp_file2 = tempfile::NamedTempFile::new().unwrap();
		let file_path2 = temp_file2.path().to_string_lossy().to_string();
		drop(temp_file2); // Delete the temp file so the path doesn't exist

		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: Some(crate::parser::ResultFormat::Table),
				to: Some(file_path2.clone()),
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		let content2 = std::fs::read_to_string(&file_path2).expect("Failed to read output file");
		assert!(!content2.is_empty());
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_no_results() {
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test \re show when no results exist
		let result = handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::Show {
				n: None,
				format: None,
				to: None,
				cols: vec![],
				limit: None,
				offset: None,
			},
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));
	}

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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test \re list with no limit (should default to 20)
		let result = crate::repl::result::handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::List {
				limit: None,
				detail: false,
			},
		)
		.await;
		assert_eq!(result, std::ops::ControlFlow::Continue(()));

		// Test \re list with limit
		let result = crate::repl::result::handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::List {
				limit: Some(2),
				detail: false,
			},
		)
		.await;
		assert_eq!(result, std::ops::ControlFlow::Continue(()));

		// Test \re list+ (detail mode)
		let result = crate::repl::result::handle_result(
			&mut ctx,
			crate::parser::ResultSubcommand::List {
				limit: None,
				detail: true,
			},
		)
		.await;
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
		)
		.await;
		assert_eq!(result, std::ops::ControlFlow::Continue(()));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_with_limit() {
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
			use_colours: false,
			vars: None,
			repl_state: &repl_state,
		};

		// Execute a query that returns multiple rows
		crate::query::execute_query(
			"SELECT n as num FROM generate_series(1, 10) n",
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test with limit=3
		let result = handle_show(&mut ctx, None, None, None, vec![], Some(3), None).await;
		assert_eq!(result, ControlFlow::Continue(()));

		// Verify the stored result still has all 10 rows (not mutated)
		{
			let state = repl_state.lock().unwrap();
			let stored = state.result_store.get_last().unwrap();
			assert_eq!(stored.rows.len(), 10);
		}
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_with_offset() {
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
			use_colours: false,
			vars: None,
			repl_state: &repl_state,
		};

		// Execute a query that returns multiple rows
		crate::query::execute_query(
			"SELECT n as num FROM generate_series(1, 10) n",
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test with offset=5
		let result = handle_show(&mut ctx, None, None, None, vec![], None, Some(5)).await;
		assert_eq!(result, ControlFlow::Continue(()));

		// Verify the stored result still has all 10 rows (not mutated)
		{
			let state = repl_state.lock().unwrap();
			let stored = state.result_store.get_last().unwrap();
			assert_eq!(stored.rows.len(), 10);
		}
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_with_limit_and_offset() {
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
			use_colours: false,
			vars: None,
			repl_state: &repl_state,
		};

		// Execute a query that returns multiple rows
		crate::query::execute_query(
			"SELECT n as num FROM generate_series(1, 10) n",
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test with offset=3 and limit=4 (should show rows 4-7)
		let result = handle_show(&mut ctx, None, None, None, vec![], Some(4), Some(3)).await;
		assert_eq!(result, ControlFlow::Continue(()));

		// Verify the stored result still has all 10 rows (not mutated)
		{
			let state = repl_state.lock().unwrap();
			let stored = state.result_store.get_last().unwrap();
			assert_eq!(stored.rows.len(), 10);
		}
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_with_cols_column() {
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
			use_colours: false,
			vars: None,
			repl_state: &repl_state,
		};

		// Execute a query with multiple columns
		crate::query::execute_query(
			"SELECT 1 as num, 'test' as text, true as flag",
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test with cols=text (should show only the text column)
		let result = handle_show(
			&mut ctx,
			None,
			None,
			None,
			vec!["text".to_string()],
			None,
			None,
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_with_cols_invalid_column() {
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
			use_colours: false,
			vars: None,
			repl_state: &repl_state,
		};

		// Execute a query
		crate::query::execute_query("SELECT 1 as num", &mut query_ctx)
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test with cols=nonexistent (should error and continue)
		let result = handle_show(
			&mut ctx,
			None,
			None,
			None,
			vec!["nonexistent".to_string()],
			None,
			None,
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_limit_offset_output() {
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
			use_colours: false,
			vars: None,
			repl_state: &repl_state,
		};

		// Execute a query that returns multiple rows with identifiable values
		crate::query::execute_query(
			"SELECT n as num FROM generate_series(1, 10) n",
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test with offset=2 and limit=3 (should show rows 3, 4, 5)
		let temp_file = tempfile::NamedTempFile::new().unwrap();
		let temp_path = temp_file.path().to_str().unwrap().to_string();
		drop(temp_file); // Delete the temp file so the path doesn't exist

		let result = handle_show(
			&mut ctx,
			None,
			None,
			Some(temp_path.clone()),
			vec![],
			Some(3),
			Some(2),
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		// Read the output file and verify it contains rows 3, 4, 5
		let output = std::fs::read_to_string(&temp_path).expect("Failed to read output file");
		assert!(output.contains(" 3"), "Output should contain row 3");
		assert!(output.contains(" 4"), "Output should contain row 4");
		assert!(output.contains(" 5"), "Output should contain row 5");
		assert!(!output.contains(" 1"), "Output should not contain row 1");
		assert!(!output.contains(" 2"), "Output should not contain row 2");
		assert!(!output.contains(" 6"), "Output should not contain row 6");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_re_show_cols_multiple_columns() {
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
			use_colours: false,
			vars: None,
			repl_state: &repl_state,
		};

		// Execute a query with multiple columns
		crate::query::execute_query("SELECT 1 as a, 2 as b, 3 as c, 4 as d", &mut query_ctx)
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

		let schema_cache_manager = crate::schema_cache::SchemaCacheManager::new(pool.clone());

		let mut ctx = crate::repl::ReplContext {
			client: &client,
			monitor_client: &monitor_client,
			backend_pid,
			theme: crate::theme::Theme::Dark,
			repl_state: &repl_state,
			rl: &mut rl,
			pool: &pool,
			schema_cache_manager: &schema_cache_manager,
		};

		// Test with cols=b,d (should show only columns b and d)
		let temp_file = tempfile::NamedTempFile::new().unwrap();
		let temp_path = temp_file.path().to_str().unwrap().to_string();
		drop(temp_file); // Delete the temp file so the path doesn't exist

		let result = handle_show(
			&mut ctx,
			None,
			None,
			Some(temp_path.clone()),
			vec!["b".to_string(), "d".to_string()],
			None,
			None,
		)
		.await;
		assert_eq!(result, ControlFlow::Continue(()));

		// Read the output file and verify it contains only b and d columns
		let output = std::fs::read_to_string(&temp_path).expect("Failed to read output file");
		assert!(
			output.contains(" b "),
			"Output should contain column b header"
		);
		assert!(
			output.contains(" d"),
			"Output should contain column d header"
		);
		assert!(
			!output.contains(" a "),
			"Output should not contain column a header"
		);
		assert!(
			!output.contains(" c"),
			"Output should not contain column c header"
		);
		assert!(
			output.contains(" 2"),
			"Output should contain value from column b"
		);
		assert!(
			output.contains(" 4"),
			"Output should contain value from column d"
		);
	}
}
