use std::{env::var, fs::File, sync::Mutex};

use clap::Subcommand;
use miette::{IntoDiagnostic, Result};
use tokio::fs::metadata;
use tracing::{info, trace, warn};

pub use context::Context;

pub mod completions;
pub mod context;
pub mod tamanu;
pub mod upload;
pub mod wifisetup;

#[derive(Debug, Clone, Subcommand)]
pub enum Action {
	Completions(completions::CompletionsArgs),
	Tamanu(tamanu::TamanuArgs),
	Upload(upload::UploadArgs),
	Wifisetup(wifisetup::WifisetupArgs),
}

pub async fn run() -> Result<()> {
	let ctx = init().await?;
	info!(version=%env!("CARGO_PKG_VERSION"), "starting up");
	trace!(?ctx, "context");

	match ctx.take_top() {
		(Action::Completions(args), ctx) => completions::run(ctx.with_top(args)).await,
		(Action::Tamanu(args), ctx) => tamanu::run(ctx.with_top(args)).await,
		(Action::Upload(args), ctx) => upload::run(ctx.with_top(args)).await,
		(Action::Wifisetup(args), ctx) => wifisetup::run(ctx.with_top(args)).await,
	}
}

async fn init() -> Result<Context<Action>> {
	let ctx = Context::new();

	let mut log_on = false;

	if !log_on && var("RUST_LOG").is_ok() {
		match tracing_subscriber::fmt()
			.with_writer(ctx.clone())
			.try_init()
		{
			Ok(_) => {
				warn!(RUST_LOG=%var("RUST_LOG").unwrap(), "logging configured from RUST_LOG");
				log_on = true;
			}
			Err(e) => eprintln!("Failed to initialise logging with RUST_LOG, falling back\n{e}"),
		}
	}

	let args = crate::args::get_args();
	let verbosity = args.verbose.unwrap_or(0);

	if log_on {
		warn!("ignoring logging options from args");
	} else if verbosity > 0 {
		let log_file = if let Some(file) = &args.log_file {
			let is_dir = metadata(&file).await.map_or(false, |info| info.is_dir());
			let path = if is_dir {
				let filename = format!(
					"bestool.{}.log",
					chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ")
				);
				file.join(filename)
			} else {
				file.to_owned()
			};

			Some(File::create(path).into_diagnostic()?)
		} else {
			None
		};

		let mut builder = tracing_subscriber::fmt().with_env_filter(match verbosity {
			0 => unreachable!("checked by if earlier"),
			1 => "warn",
			2 => "info",
			3 => "info,bestool=debug",
			4 => "debug",
			_ => "trace",
		});

		if verbosity > 2 {
			use tracing_subscriber::fmt::format::FmtSpan;
			builder = builder.with_span_events(FmtSpan::NEW | FmtSpan::CLOSE);
		}

		match if let Some(writer) = log_file {
			builder.json().with_writer(Mutex::new(writer)).try_init()
		} else if verbosity > 3 {
			builder.pretty().with_writer(ctx.clone()).try_init()
		} else {
			builder.with_writer(ctx.clone()).try_init()
		} {
			Ok(_) => info!("logging initialised"),
			Err(e) => eprintln!("Failed to initialise logging, continuing with none\n{e}"),
		}
	}

	Ok(ctx.with_top(args.action))
}
