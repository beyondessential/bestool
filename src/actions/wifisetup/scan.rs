use clap::Parser;
use miette::Result;
use tracing::instrument;

use crate::actions::Context;

use super::WifisetupArgs;

/// Scan for wifi networks.
///
/// This scans for wifi networks and prints the results to stdout. Use `--json` for machine-readable
/// output.
#[derive(Debug, Clone, Parser)]
pub struct ScanArgs {
	/// How long to wait for the scan to complete.
	///
	/// Will wait for the scan to complete, or until this timeout is reached, whichever comes first,
	/// then exit.
	#[arg(long, value_name = "DURATION", default_value = "10s")]
	pub timeout: humantime::Duration,

	/// Print output in JSON format.
	///
	/// Like the human-friendly output, one line is printed per network, as soon as it's detected.
	///
	/// {"ssid": "MyNetwork", "aps": [{"bssid":"00:11:22:33:44:55", "signal": -50}], "generation": 5, "security": "wpa2", "profile": "uuid"}
	///
	/// The "profile" field is only present if the network is already configured, and is the UUID of
	/// the connection profile.
	#[arg(long)]
	pub json: bool,

	/// Print insecure networks.
	///
	/// By default, insecure networks are not printed. This is because connecting to open wifi is
	/// not supported. Adds a "secure": false field to the JSON output.
	#[arg(long)]
	pub insecure: bool,

	/// Which interface to scan.
	///
	/// By default, the interface is autodetected.
	#[arg(long)]
	pub interface: Option<String>,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<WifisetupArgs, ScanArgs>) -> Result<()> {
	drop(ctx);
	Ok(())
}
