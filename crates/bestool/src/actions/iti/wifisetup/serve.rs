use clap::Parser;
use miette::Result;
use tracing::instrument;

use crate::actions::Context;

use super::WifisetupArgs;

/// Serve a web interface for wifi setup.
///
/// By convention, this is expected to be available at http://hostip/wifisetup.
///
/// This is a simple web interface for configuring wifi. It's intended to be used on devices that
/// don't have a screen or keyboard, and is designed to be used on a phone. It's essentially the
/// same as this CLI, but with a web interface. It's recommended to set a `--password`, otherwise
/// anyone on the same network can reconfigure the device.
#[derive(Debug, Clone, Parser)]
pub struct ServeArgs {
	/// Port to listen on.
	#[arg(long, value_name = "PORT", default_value = "9209")]
	pub port: u16,

	/// Password for the web interface.
	#[arg(long, value_name = "PASSWORD")]
	pub password: Option<String>,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<WifisetupArgs, ServeArgs>) -> Result<()> {
	drop(ctx);
	Ok(())
}
