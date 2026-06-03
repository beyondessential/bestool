use clap::Subcommand;
use miette::Result;
use tracing::{debug, trace};

pub use context::Context;
pub mod context;

/// Wire up a subcommand level.
///
/// Generates an `Action` enum (one variant per subcommand), and a `run(args,
/// ctx)` dispatcher. The `prep` closure runs before dispatch: it returns the
/// chosen `Action` along with a `Context` that has been populated with this
/// level's args (and anything else the level wants to provide). Each variant
/// then dispatches to `<modname>::run(sub_args, ctx)`.
///
/// Every level uses the same `(args, ctx)` signature, so contexts compose
/// across arbitrary nesting depth and downstream handlers can extract any
/// ancestor's args by type via `ctx.require::<T>()`.
#[macro_export]
macro_rules! subcommands {
	(
		[$argtype:ty => $prep:expr]
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

		pub async fn run(args: $argtype, ctx: Context) -> Result<()> {
			let prep = $prep;
			let (action, ctx) = prep(args, ctx)?;
			match action {
				$(
					$(#[cfg($cfg)])*
					Action::$enumname(args) => $modname::run(args, ctx).await,
				)*
			}
		}
	};
}
#[allow(unused_imports)]
pub use subcommands;

use crate::args::Args;

subcommands! {
	[Args => |args: Args, mut ctx: Context| -> Result<(Action, Context)> {
		debug!(version=%env!("CARGO_PKG_VERSION"), "starting up");
		trace!(action=?args.action, "action");
		let action = args.action.clone();
		ctx.provide(args);
		Ok((action, ctx))
	}]

	#[cfg(feature = "tamanu-psql")]
	audit_psql => AuditPsql(AuditPsqlArgs),
	#[cfg(feature = "caddy")]
	caddy => Caddy(CaddyArgs),
	#[cfg(feature = "__canopy")]
	canopy => Canopy(CanopyArgs),
	#[cfg(feature = "completions")]
	completions => Completions(CompletionsArgs),
	#[cfg(feature = "crypto")]
	crypto => Crypto(CryptoArgs),
	#[clap(hide = true)]
	#[clap(name = "_docs")]
	docs => Docs(DocsArgs),
	#[cfg(feature = "file")]
	file => File(FileArgs),
	#[cfg(feature = "__iti")]
	iti => Iti(ItiArgs),
	#[cfg(feature = "kopia")]
	#[clap(alias = "k")]
	kopia => Kopia(KopiaArgs),
	#[cfg(feature = "rdp")]
	rdp => Rdp(RdpArgs),
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
	if !matches!(action, Action::SelfUpdate(_))
		&& !crate::actions::self_update::is_package_manager_install()
		&& let Err(err) = crate::download::check_for_update().await
	{
		debug!("Failed to check for updates: {}", err);
	}

	run(args, Context::new()).await
}
