use clap::{Parser, Subcommand};
use miette::{IntoDiagnostic, Result};
use networkmanager::{
	devices::{Any, Device, WiFiDevice},
	NetworkManager,
};

use super::Context;

pub mod connect;
pub mod forget;
pub mod known;
pub mod scan;
pub mod serve;

/// Configure wifi (using NetworkManager).
#[derive(Debug, Clone, Parser)]
pub struct WifisetupArgs {
	/// Wifisetup subcommand
	#[command(subcommand)]
	pub action: WifisetupAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum WifisetupAction {
	Serve(serve::ServeArgs),
	Scan(scan::ScanArgs),
	Connect(connect::ConnectArgs),
	Forget(forget::ForgetArgs),
	Known(known::KnownArgs),
}

pub async fn run(ctx: Context<WifisetupArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		WifisetupAction::Serve(subargs) => serve::run(ctx.with_sub(subargs)).await,
		WifisetupAction::Scan(subargs) => scan::run(ctx.with_sub(subargs)).await,
		WifisetupAction::Connect(subargs) => connect::run(ctx.with_sub(subargs)).await,
		WifisetupAction::Forget(subargs) => forget::run(ctx.with_sub(subargs)).await,
		WifisetupAction::Known(subargs) => known::run(ctx.with_sub(subargs)).await,
	}
}

pub fn devices(nm: &NetworkManager, interface: Option<&str>) -> Result<Vec<WiFiDevice>> {
	let devs = nm
		.get_devices()
		.into_diagnostic()?
		.into_iter()
		.filter_map(|dev| match dev {
			Device::WiFi(dev) => Some(dev),
			_ => None,
		});

	if let Some(iface) = interface {
		Ok(devs
			.filter(|dev| dev.interface().map_or(false, |name| name == iface))
			.collect())
	} else {
		Ok(devs.collect())
	}
}
