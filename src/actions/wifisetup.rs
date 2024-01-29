use clap::{Parser, Subcommand};
use miette::{IntoDiagnostic, Result};
use networkmanager::{
	device::wireless::WirelessDevice, NetworkManager
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

pub async fn devices(nm: &NetworkManager, interface: Option<&str>) -> Result<Vec<WirelessDevice>> {
	let mut devs = Vec::new();
	for dev in nm
		.get_devices()
		.await
		.into_diagnostic()?
		{
			if let Some(wdev) = dev.to_wireless().await.into_diagnostic()? {
				devs.push((dev.interface().await.into_diagnostic()?, wdev));
			}
		}

	if let Some(iface) = interface {
		Ok(devs.into_iter()
			.filter(|(name, _)| name == iface)
			.map(|(_, dev)| dev)
			.collect())
	} else {
		Ok(devs.into_iter().map(|(_, dev)| dev).collect())
	}
}
