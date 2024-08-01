use clap::Subcommand;
use miette::Result;
use tracing::{debug, trace};

pub use context::Context;
pub mod context;

macro_rules! commands {
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

use crate::args::Args;

commands! {
	[Args => {|args: Args| -> Result<(Action, Context<()>)> {
		debug!(version=%env!("CARGO_PKG_VERSION"), "starting up");
		trace!(action=?args.action, "action");
		Ok((args.action, Context::new()))
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
	#[cfg(feature = "__tamanu")]
	tamanu => Tamanu(TamanuArgs),
	#[cfg(feature = "walg")]
	walg => WalG(WalgArgs)
}

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
					(Action::$enumname(args), ctx) => $modname::run(ctx.with_sub(args)).await,
				)*
			}
		}
	};
}
#[allow(unused_imports)]
pub(crate) use subcommands;
