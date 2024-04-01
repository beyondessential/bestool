use std::{env::var, fs::File, sync::Mutex};

use clap::Subcommand;
use miette::{IntoDiagnostic, Result};
use tokio::fs::metadata;
use tracing::{debug, trace, warn};

use crate::args::ColourMode;

pub use context::Context;
pub mod context;

#[cfg(feature = "caddy")]
pub mod caddy;
#[cfg(feature = "completions")]
pub mod completions;
#[cfg(feature = "crypto")]
pub mod crypto;
#[cfg(feature = "dyndns")]
pub mod dyndns;
#[cfg(feature = "eink")]
pub mod eink;
#[cfg(feature = "self-update")]
pub mod self_update;
#[cfg(feature = "tamanu")]
pub mod tamanu;
#[cfg(feature = "upload")]
pub mod upload;
#[cfg(feature = "walg")]
pub mod walg;
#[cfg(all(target_os = "linux", feature = "wifisetup"))]
pub mod wifisetup;

#[derive(Debug, Clone, Subcommand)]
pub enum Action {
	#[cfg(feature = "caddy")]
	Caddy(caddy::CaddyArgs),
	#[cfg(feature = "completions")]
	Completions(completions::CompletionsArgs),
	#[cfg(feature = "dyndns")]
	Dyndns(dyndns::DyndnsArgs),
	#[cfg(feature = "crypto")]
	Crypto(crypto::CryptoArgs),
	#[cfg(feature = "eink")]
	Eink(eink::EinkArgs),
	#[cfg(feature = "self-update")]
	SelfUpdate(self_update::SelfUpdateArgs),
	#[cfg(feature = "tamanu")]
	Tamanu(tamanu::TamanuArgs),
	#[cfg(feature = "upload")]
	Upload(upload::UploadArgs),
	#[cfg(feature = "walg")]
	WalG(walg::WalgArgs),
	#[cfg(all(target_os = "linux", feature = "wifisetup"))]
	Wifisetup(wifisetup::WifisetupArgs),
}

pub async fn run() -> Result<()> {
	let ctx = init().await?;
	debug!(version=%env!("CARGO_PKG_VERSION"), "starting up");
	trace!(?ctx, "context");

	match ctx.take_top() {
		#[cfg(feature = "caddy")]
		(Action::Caddy(args), ctx) => caddy::run(ctx.with_top(args)).await,
		#[cfg(feature = "completions")]
		(Action::Completions(args), ctx) => completions::run(ctx.with_top(args)).await,
		#[cfg(feature = "dyndns")]
		(Action::Dyndns(args), ctx) => dyndns::run(ctx.with_top(args)).await,
		#[cfg(feature = "crypto")]
		(Action::Crypto(args), ctx) => crypto::run(ctx.with_top(args)).await,
		#[cfg(feature = "eink")]
		(Action::Eink(args), ctx) => eink::run(ctx.with_top(args)).await,
		#[cfg(feature = "self-update")]
		(Action::SelfUpdate(args), ctx) => self_update::run(ctx.with_top(args)).await,
		#[cfg(feature = "tamanu")]
		(Action::Tamanu(args), ctx) => tamanu::run(ctx.with_top(args)).await,
		#[cfg(feature = "upload")]
		(Action::Upload(args), ctx) => upload::run(ctx.with_top(args)).await,
		#[cfg(feature = "walg")]
		(Action::WalG(args), ctx) => walg::run(ctx.with_top(args)).await,
		#[cfg(all(target_os = "linux", feature = "wifisetup"))]
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
	} else {
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
			0 => "info",
			1 => "info,bestool=debug",
			2 => "debug",
			3 => "debug,bestool=trace",
			_ => "trace",
		});

		match args.color {
			ColourMode::Never => {
				builder = builder.with_ansi(false);
			}
			ColourMode::Always => {
				builder = builder.with_ansi(true);
			}
			ColourMode::Auto => {}
		}

		if verbosity > 0 {
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
			Ok(_) => debug!("logging initialised"),
			Err(e) => eprintln!("Failed to initialise logging, continuing with none\n{e}"),
		}
	}

	Ok(ctx.with_top(args.action))
}
