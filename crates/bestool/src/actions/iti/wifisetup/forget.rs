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
	/// SSID of the wifi network to forget.
	///
	/// Obtain it using `bestool wifisetup known`.
	#[arg(long, value_name = "SSID")]
	pub ssid: String,

	/// Which interface to change.
	///
	/// By default, all interfaces are affected.
	#[arg(long)]
	pub interface: Option<String>,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<WifisetupArgs, ForgetArgs>) -> Result<()> {
	drop(ctx);
	Ok(())
}
