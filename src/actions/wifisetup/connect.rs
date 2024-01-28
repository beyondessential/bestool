use clap::Parser;
use miette::Result;
use tracing::instrument;

use crate::actions::Context;

use super::WifisetupArgs;

/// Create a wifi connection.
///
/// This creates a new connection profile. The wifi network doesn't need to be currently
/// broadcasting; it will be used the next time the device is in range.
///
/// If either `--name` or `--id` is provided, and a matching connection profile already exists, it
/// will be updated with the new settings. Otherwise, a new profile will be created.
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

	/// Name of the connection profile.
	///
	/// If a profile with this name already exists, it will be updated with the new settings.
	#[arg(long, value_name = "NAME")]
	pub name: Option<String>,

	/// UUID of the connection profile.
	///
	/// If a profile with this UUID already exists, it will be updated with the new settings.
	#[arg(long, value_name = "UUID")]
	pub id: Option<String>,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<WifisetupArgs, ConnectArgs>) -> Result<()> {
	drop(ctx);
	Ok(())
}
