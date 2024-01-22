use std::{env::var, fs::File, sync::Mutex};

use clap::Subcommand;
use miette::{IntoDiagnostic, Result};
use tokio::fs::metadata;
use tracing::{debug, info, warn};

pub mod completions;
pub mod tamanu;
pub mod upload;

#[derive(Debug, Clone, Subcommand)]
pub enum Action {
	Completions(completions::CompletionsArgs),
	Tamanu(tamanu::TamanuArgs),
	Upload(upload::UploadArgs),
}

pub async fn run() -> Result<()> {
	let args = init().await?;
	info!(version=%env!("CARGO_PKG_VERSION"), "starting up");
	debug!(?args, "arguments");

	match args.action {
		Action::Completions(args) => completions::run(args).await,
		Action::Tamanu(args) => tamanu::run(args).await,
		Action::Upload(args) => upload::run(args).await,
	}
}

async fn init() -> Result<crate::args::Args> {
	let mut log_on = false;

	if !log_on && var("RUST_LOG").is_ok() {
		match tracing_subscriber::fmt::try_init() {
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
			3 => "debug",
			_ => "trace",
		});

		if verbosity > 2 {
			use tracing_subscriber::fmt::format::FmtSpan;
			builder = builder.with_span_events(FmtSpan::NEW | FmtSpan::CLOSE);
		}

		match if let Some(writer) = log_file {
			builder.json().with_writer(Mutex::new(writer)).try_init()
		} else if verbosity > 3 {
			builder.pretty().try_init()
		} else {
			builder.try_init()
		} {
			Ok(_) => info!("logging initialised"),
			Err(e) => eprintln!("Failed to initialise logging, continuing with none\n{e}"),
		}
	}

	Ok(args)
}
