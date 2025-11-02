use std::ops::ControlFlow;

use tokio::{fs::File, io};
use tracing::{error, warn};

use super::{state::ReplContext, transaction::TransactionState};
use crate::{error::format_miette_error, parser::QueryModifier, query::execute_query};

pub async fn handle_execute(
	ctx: &mut ReplContext<'_>,
	_input: String,
	sql: String,
	modifiers: crate::parser::QueryModifiers,
) -> ControlFlow<()> {
	let output_file_path = modifiers.iter().find_map(|m| {
		if let QueryModifier::Output { file_path } = m {
			Some(file_path.clone())
		} else {
			None
		}
	});

	let use_colours =
		if output_file_path.is_some() || ctx.repl_state.lock().unwrap().output_file.is_some() {
			false
		} else {
			ctx.repl_state.lock().unwrap().use_colours
		};

	let result = if let Some(path) = output_file_path {
		if std::path::Path::new(&path).exists() {
			error!("File already exists: {path}");
			eprintln!("Error: File already exists: {}", path);
			return ControlFlow::Continue(());
		}

		match File::create(&path).await {
			Ok(mut file) => {
				let mut vars = {
					let state = ctx.repl_state.lock().unwrap();
					state.vars.clone()
				};
				let mut query_ctx = crate::query::QueryContext {
					client: ctx.client,
					modifiers: modifiers.clone(),
					theme: ctx.theme,
					writer: &mut file,
					use_colours,
					vars: Some(&mut vars),
					repl_state: ctx.repl_state,
				};
				let result = execute_query(&sql, &mut query_ctx).await;
				ctx.repl_state.lock().unwrap().vars = vars;
				result
			}
			Err(e) => {
				error!("Failed to open output file '{path}': {e}");
				return ControlFlow::Continue(());
			}
		}
	} else {
		let file_arc_opt = ctx.repl_state.lock().unwrap().output_file.clone();
		if let Some(file_arc) = file_arc_opt {
			let mut vars = {
				let state = ctx.repl_state.lock().unwrap();
				state.vars.clone()
			};

			let mut file = file_arc.lock().await;
			let mut query_ctx = crate::query::QueryContext {
				client: ctx.client,
				modifiers: modifiers.clone(),
				theme: ctx.theme,
				writer: &mut *file,
				use_colours,
				vars: Some(&mut vars),
				repl_state: ctx.repl_state,
			};
			let result = execute_query(&sql, &mut query_ctx).await;

			ctx.repl_state.lock().unwrap().vars = vars;
			result
		} else {
			let mut stdout = io::stdout();
			let mut vars = {
				let state = ctx.repl_state.lock().unwrap();
				state.vars.clone()
			};
			let mut query_ctx = crate::query::QueryContext {
				client: ctx.client,
				modifiers,
				theme: ctx.theme,
				writer: &mut stdout,
				use_colours,
				vars: Some(&mut vars),
				repl_state: ctx.repl_state,
			};
			let result = execute_query(&sql, &mut query_ctx).await;
			ctx.repl_state.lock().unwrap().vars = vars;
			result
		}
	};

	match result {
		Ok(()) => {
			let tx_state = TransactionState::check(ctx.monitor_client, ctx.backend_pid).await;
			if ctx.repl_state.lock().unwrap().write_mode
				&& matches!(tx_state, TransactionState::None)
				&& let Err(e) = ctx.client.batch_execute("BEGIN").await
			{
				warn!("Failed to start transaction: {e}");
			}
		}
		Err(e) => {
			eprintln!("{}", format_miette_error(&e, None));
		}
	}

	ControlFlow::Continue(())
}
