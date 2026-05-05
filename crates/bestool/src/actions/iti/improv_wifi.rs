use std::time::Duration;

use clap::Parser;
use improv_wifi::{
	AuthorizeMode, Connection, ImprovWifi, ImprovWifiConfig, find_adapter,
	networkmanager::NetworkManagerBackend, power_on_adapter,
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

	let connection = Connection::system()
		.await
		.into_diagnostic()
		.wrap_err("zbus: connect to system bus")?;

	let adapter_path = find_adapter(&connection, args.adapter.as_deref())
		.await
		.into_diagnostic()
		.wrap_err("BlueZ: locate adapter")?;

	power_on_adapter(&connection, &adapter_path)
		.await
		.into_diagnostic()
		.wrap_err("BlueZ: power on adapter")?;

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
	let service = ImprovWifi::install(connection, adapter_path, backend, config)
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
