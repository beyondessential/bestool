use std::ops::ControlFlow;

use super::state::ReplContext;

pub async fn handle_debug(
	ctx: &mut ReplContext<'_>,
	what: crate::parser::DebugWhat,
) -> ControlFlow<()> {
	use crate::parser::DebugWhat;

	match what {
		DebugWhat::State => {
			let state = ctx.repl_state.lock().unwrap();
			eprintln!("ReplState: {:#?}", *state);
		}
		DebugWhat::RefreshSchema => {
			eprintln!("Refreshing schema cache...");
			match ctx.schema_cache_manager.refresh().await {
				Ok(()) => eprintln!("Schema cache refreshed successfully"),
				Err(e) => eprintln!("Failed to refresh schema cache: {e}"),
			}
		}
		DebugWhat::Redactions => {
			let state = ctx.repl_state.lock().unwrap();
			if state.config.redactions.is_empty() {
				eprintln!("No redactions configured.");
			} else {
				let mut table = comfy_table::Table::new();
				crate::table::configure(&mut table);
				table.load_preset(comfy_table::presets::NOTHING);

				eprintln!("Redactions ({} total):", state.config.redactions.len());

				let mut sorted: Vec<_> = state.config.redactions.iter().collect();
				sorted.sort_by(|a, b| {
					a.schema
						.cmp(&b.schema)
						.then_with(|| a.table.cmp(&b.table))
						.then_with(|| a.column.cmp(&b.column))
				});

				for redaction in sorted {
					table.add_row(vec![&redaction.schema, &redaction.table, &redaction.column]);
				}

				eprintln!("{table}");
			}
		}
		DebugWhat::Help => {
			eprintln!("Available debug commands:");
			eprintln!("  \\debug state           - Show current REPL state");
			eprintln!("  \\debug refresh-schema  - Refresh schema cache (for completion)");
			eprintln!("  \\debug redactions      - Show configured redactions");
		}
	}

	ControlFlow::Continue(())
}
