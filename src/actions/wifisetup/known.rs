use clap::Parser;
use miette::{IntoDiagnostic, Result};
use networkmanager::{NetworkManager};
use tracing::instrument;

use crate::actions::Context;

use super::{devices, WifisetupArgs};

/// List known wifi networks.
///
/// This lists wifi networks for which a configuration exists.
#[derive(Debug, Clone, Parser)]
pub struct KnownArgs {
	/// Which interface to filter to.
	///
	/// By default, all interfaces are used.
	#[arg(long)]
	pub interface: Option<String>,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<WifisetupArgs, KnownArgs>) -> Result<()> {
	let nm = NetworkManager::new().await.into_diagnostic()?;
	let devs = devices(&nm, ctx.args_sub.interface.as_deref()).await?;

	for conn in nm.settings().list_connections().await.into_diagnostic()? {
		dbg!(conn);
	}

	Ok(())
}
