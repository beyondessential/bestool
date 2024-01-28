use clap::Parser;
use miette::Result;
use tracing::instrument;

use crate::actions::Context;

use super::WifisetupArgs;

/// Create a wifi connection.
///
/// This creates a new connection profile if needed. The wifi network doesn't need to be currently
/// broadcasting; it will be used the next time the device is in range.
#[derive(Debug, Clone, Parser)]
pub struct ConnectArgs {
	/// SSID of the wifi network.
	///
	/// Obtain it using `bestool wifisetup scan`.
	#[arg(long, value_name = "SSID")]
	pub ssid: String,

	/// Password for the wifi network.
	///
	/// Connecting to open wifi networks is not supported as these are insecure.
	#[arg(long, value_name = "PASSWORD")]
	pub password: String,

	/// Which interface to use.
	///
	/// By default, the interface is autodetected.
	#[arg(long)]
	pub interface: Option<String>,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<WifisetupArgs, ConnectArgs>) -> Result<()> {
	drop(ctx);
	Ok(())
}
