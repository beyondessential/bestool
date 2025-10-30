use crossterm::style::{Color, Stylize};
use miette::{IntoDiagnostic, Result};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tracing::{debug, warn};

use crate::{
	parser::{QueryModifier, QueryModifiers},
	signals::{reset_sigint, sigint_received},
};

mod column;
mod display;
mod vars;

/// Context for executing a query.
pub(crate) struct QueryContext<'a, W: AsyncWrite + Unpin> {
	pub client: &'a tokio_postgres::Client,
	pub modifiers: QueryModifiers,
	pub theme: crate::theme::Theme,
	pub writer: &'a mut W,
	pub use_colours: bool,
	pub vars: Option<&'a mut std::collections::BTreeMap<String, String>>,
}

/// Execute a SQL query and display the results.
pub(crate) async fn execute_query<W: AsyncWrite + Unpin>(
	sql: &str,
	ctx: &mut QueryContext<'_, W>,
) -> Result<()> {
	debug!(?ctx.modifiers, %sql, "executing query");

	// Interpolate variables unless Verbatim modifier is used
	let sql_to_execute = if ctx.modifiers.contains(&QueryModifier::Verbatim) {
		sql.to_string()
	} else {
		let empty_vars = std::collections::BTreeMap::new();
		let vars = ctx.vars.as_ref().map_or(&empty_vars, |v| v);
		vars::interpolate_variables(sql, vars)?
	};

	let start = std::time::Instant::now();

	let cancel_token = ctx.client.cancel_token();
	let tls_connector = crate::tls::make_tls_connector()?;

	// Reset the flag before starting
	reset_sigint();

	// Poll for SIGINT while executing query
	let result = tokio::select! {
		result = ctx.client.query(&sql_to_execute, &[]) => {
			result.into_diagnostic()
		}
		_ = async {
			loop {
				tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
				if sigint_received() {
					break;
				}
			}
		} => {
			eprintln!("\nCancelling query...");
			if let Err(e) = cancel_token.cancel_query(tls_connector).await {
				warn!("Failed to cancel query: {:?}", e);
			}
			// Reset flag for next time
			reset_sigint();
			return Ok(());
		}
	};

	let rows = result?;

	let duration = start.elapsed();

	if rows.is_empty() {
		let msg = if ctx.use_colours {
			format!("{}\n", "(no rows)".with(Color::Blue).dim())
		} else {
			"(no rows)\n".to_string()
		};
		ctx.writer
			.write_all(msg.as_bytes())
			.await
			.into_diagnostic()?;
		ctx.writer.flush().await.into_diagnostic()?;
		return Ok(());
	}

	if let Some(first_row) = rows.first() {
		let columns = first_row.columns();

		let mut unprintable_columns = Vec::new();
		for (i, _column) in columns.iter().enumerate() {
			if !column::can_print(first_row, i) {
				unprintable_columns.push(i);
			}
		}

		let text_rows = if !unprintable_columns.is_empty() {
			let sql_trimmed = sql_to_execute.trim_end_matches(';').trim();
			let text_query = build_text_cast_query(sql_trimmed, columns, &unprintable_columns);
			debug!("re-querying with text casts: {text_query}");
			match ctx.client.query(&text_query, &[]).await {
				Ok(rows) => Some(rows),
				Err(e) => {
					debug!("failed to re-query with text casts: {e:?}");
					None
				}
			}
		} else {
			None
		};

		let is_expanded = ctx.modifiers.contains(&QueryModifier::Expanded);
		let is_json = ctx.modifiers.contains(&QueryModifier::Json);

		display::display(
			&mut display::DisplayContext {
				columns,
				rows: &rows,
				unprintable_columns: &unprintable_columns,
				text_rows: &text_rows,
				writer: ctx.writer,
				use_colours: ctx.use_colours,
				theme: ctx.theme,
			},
			is_json,
			is_expanded,
		)
		.await?;

		let status_text = format!(
			"({} row{}, took {:.3}ms)",
			rows.len(),
			if rows.len() == 1 { "" } else { "s" },
			duration.as_secs_f64() * 1000.0
		);
		let status_msg = if ctx.use_colours {
			format!("{}\n", status_text.with(Color::Blue).dim())
		} else {
			format!("{status_text}\n")
		};
		// Status messages always go to stderr
		eprint!("{status_msg}");

		// Handle VarSet modifier: if exactly one row, store column values as variables
		if rows.len() == 1 {
			if let Some(var_prefix) = ctx.modifiers.iter().find_map(|m| {
				if let QueryModifier::VarSet { prefix } = m {
					Some(prefix)
				} else {
					None
				}
			}) {
				if let Some(vars_map) = ctx.vars.as_mut() {
					let row = &rows[0];
					for (i, column) in columns.iter().enumerate() {
						let var_name = if let Some(prefix_str) = var_prefix {
							format!("{prefix_str}{col_name}", col_name = column.name())
						} else {
							column.name().to_string()
						};

						// Get the value as a string
						let value = if unprintable_columns.contains(&i) {
							if let Some(ref text_rows) = text_rows {
								if let Some(text_row) = text_rows.first() {
									text_row
										.try_get::<_, String>(i)
										.unwrap_or_else(|_| String::new())
								} else {
									String::new()
								}
							} else {
								String::new()
							}
						} else {
							column::get_value(row, i, 0, &unprintable_columns, &text_rows)
						};

						vars_map.insert(var_name, value);
					}
				}
			}
		}
	}

	Ok(())
}

fn build_text_cast_query(
	sql: &str,
	columns: &[tokio_postgres::Column],
	unprintable_columns: &[usize],
) -> String {
	let column_exprs: Vec<String> = columns
		.iter()
		.enumerate()
		.map(|(i, col)| {
			if unprintable_columns.contains(&i) {
				format!("(subq.{col_name})::text", col_name = col.name())
			} else {
				format!("subq.{col_name}", col_name = col.name())
			}
		})
		.collect();

	format!(
		"SELECT {cols} FROM ({sql}) AS subq",
		cols = column_exprs.join(", ")
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_build_text_cast_query_logic() {
		let sql = "SELECT id, name, data FROM users";
		let column_names = vec!["id", "name", "data"];
		let unprintable_indices = vec![0, 2];

		let column_exprs: Vec<String> = column_names
			.iter()
			.enumerate()
			.map(|(i, name)| {
				if unprintable_indices.contains(&i) {
					format!("(subq.{name})::text")
				} else {
					format!("subq.{name}")
				}
			})
			.collect();

		let result = format!(
			"SELECT {cols} FROM ({sql}) AS subq",
			cols = column_exprs.join(", ")
		);

		assert!(result.contains("(subq.id)::text"));
		assert!(result.contains("subq.name"));
		assert!(result.contains("(subq.data)::text"));
		assert!(result.starts_with("SELECT"));
		assert!(result.contains("AS subq"));
	}

	#[test]
	fn test_expanded_modifier_detection() {
		use std::collections::HashSet;

		// Test with expanded modifier
		let mut modifiers_with_expanded = HashSet::new();
		modifiers_with_expanded.insert(QueryModifier::Expanded);
		assert!(modifiers_with_expanded.contains(&QueryModifier::Expanded));

		// Test without expanded modifier
		let mut modifiers_without_expanded = HashSet::new();
		modifiers_without_expanded.insert(QueryModifier::Json);
		assert!(!modifiers_without_expanded.contains(&QueryModifier::Expanded));

		// Test with multiple modifiers including expanded
		let mut modifiers_mixed = HashSet::new();
		modifiers_mixed.insert(QueryModifier::Expanded);
		modifiers_mixed.insert(QueryModifier::Json);
		assert!(modifiers_mixed.contains(&QueryModifier::Expanded));
	}

	#[test]
	fn test_json_modifier_detection() {
		use std::collections::HashSet;

		let mut modifiers_with_json = HashSet::new();
		modifiers_with_json.insert(QueryModifier::Json);
		assert!(modifiers_with_json.contains(&QueryModifier::Json));

		let mut modifiers_without_json = HashSet::new();
		modifiers_without_json.insert(QueryModifier::Expanded);
		assert!(!modifiers_without_json.contains(&QueryModifier::Json));

		let mut modifiers_both = HashSet::new();
		modifiers_both.insert(QueryModifier::Json);
		modifiers_both.insert(QueryModifier::Expanded);
		assert!(modifiers_both.contains(&QueryModifier::Json));
		assert!(modifiers_both.contains(&QueryModifier::Expanded));
	}

	#[test]
	fn test_execute_query_with_verbatim_modifier() {
		// Test that Verbatim modifier prevents interpolation
		let mut mods: QueryModifiers = QueryModifiers::new();
		mods.insert(QueryModifier::Verbatim);

		assert!(mods.contains(&QueryModifier::Verbatim));
	}

	#[test]
	fn test_execute_query_without_verbatim_modifier() {
		// Test that without Verbatim modifier, interpolation would occur
		let mods: QueryModifiers = QueryModifiers::new();

		assert!(!mods.contains(&QueryModifier::Verbatim));
	}
}
