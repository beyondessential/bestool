use std::{ops::ControlFlow, path::Path};

use tokio::{fs::File, io::AsyncWriteExt, sync::Mutex as TokioMutex};
use tracing::{debug, error, warn};

use super::state::ReplContext;
use std::sync::Arc;

pub async fn handle_set_output(ctx: &mut ReplContext<'_>, file_path: &Path) -> ControlFlow<()> {
	let _ = handle_unset_output(ctx).await;

	match File::create(file_path).await {
		Ok(file) => {
			debug!("opened output file: {file_path:?}");
			eprintln!("Output will be written to: {file_path:?}");
			let mut state = ctx.repl_state.lock().unwrap();
			state.output_file = Some(Arc::new(TokioMutex::new(file)));
		}
		Err(e) => {
			error!("Failed to open output file '{file_path:?}': {e}");
		}
	}

	ControlFlow::Continue(())
}

pub async fn handle_unset_output(ctx: &mut ReplContext<'_>) -> ControlFlow<()> {
	let file_arc_opt = {
		let mut state = ctx.repl_state.lock().unwrap();
		state.output_file.take()
	};

	if let Some(file_arc) = file_arc_opt {
		let mut file = file_arc.lock().await;
		if let Err(e) = file.flush().await {
			warn!("failed to flush output file: {e}");
		}
		debug!("closed output file");
		eprintln!("Output redirection closed");
	}

	ControlFlow::Continue(())
}
