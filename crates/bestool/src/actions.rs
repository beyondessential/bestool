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
			$(#[cfg($cfg:meta)])*
			$(#[clap($clap:meta)])*
			$modname:ident => $enumname:ident($argname:ident)
		),+
	) => {
		$(
			$(#[cfg($cfg)])*
			pub mod $modname;
		)*

		#[derive(Debug, Clone, Subcommand)]
		pub enum Action {
			$(
				$(#[cfg($cfg)])*
				$(#[clap($clap)])*
				$enumname($modname::$argname),
			)*
		}

		pub async fn run(ctx: $argtype) -> Result<()> {
			let ctxfn = $ctxcode;
			match ctxfn(ctx)? {
				$(
					$(#[cfg($cfg)])*
					(Action::$enumname(args), ctx) => $modname::run(ctx.$ctxmethod(args)).await,
				)*
			}
		}
	};
}
#[allow(unused_imports)]
pub use subcommands;

use crate::args::Args;

subcommands! {
	[Args => {|args: Args| -> Result<(Action, Context<Args>)> {
		debug!(version=%env!("CARGO_PKG_VERSION"), "starting up");
		trace!(action=?args.action, "action");
		Ok((args.action.clone(), Context::new().with_top(args)))
	}}](with_sub)

	#[cfg(feature = "caddy")]
	caddy => Caddy(CaddyArgs),
	#[cfg(feature = "completions")]
	completions => Completions(CompletionsArgs),
	#[cfg(feature = "crypto")]
	crypto => Crypto(CryptoArgs),
	#[cfg(feature = "file")]
	file => File(FileArgs),
	#[cfg(feature = "__iti")]
	iti => Iti(ItiArgs),
	#[cfg(feature = "self-update")]
	#[clap(alias = "self")]
	self_update => SelfUpdate(SelfUpdateArgs),
	#[cfg(feature = "ssh")]
	ssh => Ssh(SshArgs),
	#[cfg(feature = "__tamanu")]
	#[clap(alias = "t")]
	tamanu => Tamanu(TamanuArgs)
}

pub async fn run_with_update_check(args: Args) -> Result<()> {
	let action = args.action.clone();

	#[cfg(all(feature = "download", feature = "self-update"))]
	if !matches!(action, Action::SelfUpdate(_)) {
		if let Err(err) = crate::download::check_for_update().await {
			debug!("Failed to check for updates: {}", err);
		}
	}

	run(args).await
}
