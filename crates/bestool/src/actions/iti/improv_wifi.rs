use std::time::Duration;

use clap::Parser;
use improv_wifi::{
	AuthorizeMode, ImprovWifi, ImprovWifiConfig, networkmanager::NetworkManagerBackend,
};
use miette::{IntoDiagnostic, Result, WrapErr};
use tracing::info;

use crate::actions::{Context, iti::ItiArgs};

/// Run the Improv-Wi-Fi BLE peripheral so a phone or browser can provision the device's Wi-Fi.
///
/// Uses BlueZ for BLE and NetworkManager for Wi-Fi configuration. The service exits cleanly once
/// the device has been successfully provisioned.
#[derive(Debug, Clone, Parser)]
pub struct ImprovWifiArgs {
	/// Bluetooth adapter to use (e.g. `hci0`). Defaults to the system's first powered adapter.
	#[arg(long)]
	pub adapter: Option<String>,

	/// Local name advertised over BLE. Defaults to the system hostname.
	#[arg(long)]
	pub local_name: Option<String>,

	/// Device name reported in Device Info / Device Name commands. Defaults to the system hostname.
	#[arg(long)]
	pub device_name: Option<String>,

	/// Require explicit authorization before accepting credentials.
	///
	/// Without this flag the device starts already in `Authorized` and accepts credentials
	/// immediately. With this flag the device starts in `AuthorizationRequired` and there is
	/// currently no in-tool mechanism to authorize it (a follow-up will add a GPIO-button hook).
	#[arg(long)]
	pub require_authorization: bool,

	/// Authorization timeout. Only meaningful with `--require-authorization`.
	#[arg(long, default_value = "60s")]
	pub auth_timeout: humantime::Duration,
}

pub async fn run(ctx: Context<ItiArgs, ImprovWifiArgs>) -> Result<()> {
	let args = ctx.args_sub;

	let session = bluer::Session::new()
		.await
		.into_diagnostic()
		.wrap_err("bluer: open session")?;

	let adapter = match args.adapter.as_deref() {
		Some(name) => session
			.adapter(name)
			.into_diagnostic()
			.wrap_err_with(|| format!("bluer: adapter {name}"))?,
		None => session
			.default_adapter()
			.await
			.into_diagnostic()
			.wrap_err("bluer: default adapter")?,
	};
	adapter
		.set_powered(true)
		.await
		.into_diagnostic()
		.wrap_err("bluer: power on adapter")?;

	let hostname = read_hostname();
	let device_name = args.device_name.unwrap_or_else(|| hostname.clone());
	let local_name = args.local_name.or(Some(hostname));

	let backend = NetworkManagerBackend::new(device_name)
		.await
		.into_diagnostic()
		.wrap_err("improv-wifi: connect to NetworkManager")?;

	let config = ImprovWifiConfig {
		authorize: if args.require_authorization {
			AuthorizeMode::Required
		} else {
			AuthorizeMode::NotRequired
		},
		auth_timeout: Duration::from(args.auth_timeout),
		local_name,
	};

	info!("starting Improv-Wi-Fi service");
	let service = ImprovWifi::install(&adapter, backend, config)
		.await
		.into_diagnostic()
		.wrap_err("improv-wifi: install GATT service")?;

	service
		.run()
		.await
		.into_diagnostic()
		.wrap_err("improv-wifi: service loop")?;

	info!("Improv-Wi-Fi service finished");
	Ok(())
}

fn read_hostname() -> String {
	std::fs::read_to_string("/etc/hostname")
		.ok()
		.map(|s| s.trim().to_owned())
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| "improv-device".into())
}
