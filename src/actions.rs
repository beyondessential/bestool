use std::{
	env::var,
	fs::{metadata, File},
	sync::Mutex,
};

use clap::Subcommand;
use miette::{IntoDiagnostic, Result};
use tracing::{debug, trace, warn};

use crate::args::ColourMode;

pub use context::Context;
pub mod context;

#[macro_export]
macro_rules! subcommands {
	(
		[$argtype:ty => $ctxcode:block]
		$(
			#[$meta:meta]
			$modname:ident => $enumname:ident($argname:ident)
		),+
	) => {
		$(
			#[$meta]
			pub mod $modname;
		)*

		#[derive(Debug, Clone, Subcommand)]
		pub enum Action {
			$(
				#[$meta]
				$enumname($modname::$argname),
			)*
		}

		pub async fn run(ctx: $argtype) -> Result<()> {
			let ctxfn = $ctxcode;
			match ctxfn(ctx)? {
				$(
					#[$meta]
					(Action::$enumname(args), ctx) => $modname::run(ctx.with_top(args)).await,
				)*
			}
		}
	};
}
pub(crate) use subcommands;

subcommands! {
	[() => {|_ctx: ()| -> Result<(Action, Context<()>)> {
		let ctx = init()?;
		debug!(version=%env!("CARGO_PKG_VERSION"), "starting up");
		trace!(?ctx, "context");
		Ok(ctx.take_top())
	}}]

	#[cfg(feature = "caddy")]
	caddy => Caddy(CaddyArgs),
	#[cfg(feature = "completions")]
	completions => Completions(CompletionsArgs),
	#[cfg(feature = "crypto")]
	crypto => Crypto(CryptoArgs),
	#[cfg(feature = "dyndns")]
	dyndns => Dyndns(DyndnsArgs),
	#[cfg(feature = "__iti")]
	iti => Iti(ItiArgs),
	#[cfg(feature = "self-update")]
	self_update => SelfUpdate(SelfUpdateArgs),
	#[cfg(feature = "ssh")]
	ssh => Ssh(SshArgs),
	#[cfg(feature = "tamanu")]
	tamanu => Tamanu(TamanuArgs),
	#[cfg(feature = "upload")]
	upload => Upload(UploadArgs),
	#[cfg(feature = "walg")]
	walg => WalG(WalgArgs)
}

fn init() -> Result<Context<Action>> {
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
			let is_dir = metadata(&file).map_or(false, |info| info.is_dir());
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
