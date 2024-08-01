use clap::Subcommand;
use miette::Result;
use tracing::{debug, trace};

pub use context::Context;
pub mod context;

#[macro_export]
macro_rules! subcommands {
	(
		[$argtype:ty => $ctxcode:block]($ctxmethod:ident)
		$(
			$(#[$meta:meta])*
			$modname:ident => $enumname:ident($argname:ident)
		),+
	) => {
		$(
			$(#[$meta])*
			pub mod $modname;
		)*

		#[derive(Debug, Clone, Subcommand)]
		pub enum Action {
			$(
				$(#[$meta])*
				$enumname($modname::$argname),
			)*
		}

		pub async fn run(ctx: $argtype) -> Result<()> {
			let ctxfn = $ctxcode;
			match ctxfn(ctx)? {
				$(
					$(#[$meta])*
					(Action::$enumname(args), ctx) => $modname::run(ctx.$ctxmethod(args)).await,
				)*
			}
		}
	};
}
#[allow(unused_imports)]
pub(crate) use subcommands;

use crate::args::Args;

subcommands! {
	[Args => {|args: Args| -> Result<(Action, Context<()>)> {
		debug!(version=%env!("CARGO_PKG_VERSION"), "starting up");
		trace!(action=?args.action, "action");
		Ok((args.action, Context::new()))
	}}](with_top)

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
