use clap::Parser;
use miette::Result;
use tracing::instrument;

use crate::actions::Context;

use super::WifisetupArgs;

/// Forget a wifi connection.
///
/// This deletes the connection profile and reloads the network configuration. Be careful if you're
/// connected over wifi, as this may disconnect you.
#[derive(Debug, Clone, Parser)]
pub struct ForgetArgs {
	/// Name of the connection profile.
	///
	/// Either this or `--id` must be provided.
	#[arg(long, value_name = "NAME", required_unless_present = "id")]
	pub name: Option<String>,

	/// UUID of the connection profile.
	///
	/// Either this or `--name` must be provided.
	#[arg(long, value_name = "UUID", required_unless_present = "name")]
	pub id: Option<String>,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<WifisetupArgs, ForgetArgs>) -> Result<()> {
	drop(ctx);
	Ok(())
}
