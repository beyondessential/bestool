use std::time::Duration;

use clap::Parser;
use improv_wifi::{
	AuthHandle, AuthorizeMode, Connection, ImprovWifi, ImprovWifiConfig, find_adapter,
	networkmanager::NetworkManagerBackend, power_on_adapter,
};
use miette::{IntoDiagnostic, Result, WrapErr};
use rppal::gpio::{Gpio, Trigger};
use tokio::io::{AsyncBufReadExt, BufReader};
use tracing::{debug, info, warn};

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

	/// Authorize when a line is received on stdin (the line content is ignored).
	///
	/// When set, the device starts in `AuthorizationRequired` and only accepts credentials
	/// after the first line on stdin.
	#[arg(long)]
	pub auth_stdin: bool,

	/// Authorize on a button press on this BCM GPIO pin.
	///
	/// The pin is configured as input with the internal pull-up resistor; wire a momentary
	/// switch from the pin to GND. When set, the device starts in `AuthorizationRequired`
	/// and only accepts credentials after the first press.
	#[arg(long)]
	pub auth_gpio: Option<u8>,

	/// Debounce window for `--auth-gpio`.
	#[arg(long, default_value = "50ms")]
	pub auth_gpio_debounce: humantime::Duration,

	/// How long an authorization stays valid before the device reverts to
	/// `AuthorizationRequired`. Only meaningful with `--auth-stdin` / `--auth-gpio`.
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

	let auth_required = args.auth_stdin || args.auth_gpio.is_some();

	let config = ImprovWifiConfig {
		authorize: if auth_required {
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

	let auth_handle = service.auth_handle();

	let stdin_task = args.auth_stdin.then(|| spawn_stdin_authorizer(auth_handle.clone()));

	let _gpio_pin = if let Some(pin) = args.auth_gpio {
		Some(
			install_gpio_authorizer(pin, args.auth_gpio_debounce.into(), auth_handle)
				.wrap_err_with(|| format!("auth-gpio: configure BCM pin {pin}"))?,
		)
	} else {
		None
	};

	let result = service
		.run()
		.await
		.into_diagnostic()
		.wrap_err("improv-wifi: service loop");

	if let Some(handle) = stdin_task {
		handle.abort();
	}

	result?;

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

fn spawn_stdin_authorizer(auth: AuthHandle) -> tokio::task::JoinHandle<()> {
	tokio::spawn(async move {
		let mut lines = BufReader::new(tokio::io::stdin()).lines();
		loop {
			match lines.next_line().await {
				Ok(Some(_)) => {
					info!("stdin line received, authorizing");
					auth.authorize();
				}
				Ok(None) => {
					debug!("stdin closed, ending stdin authorizer");
					break;
				}
				Err(err) => {
					warn!(?err, "stdin read error, ending stdin authorizer");
					break;
				}
			}
		}
	})
}

fn install_gpio_authorizer(
	pin: u8,
	debounce: Duration,
	auth: AuthHandle,
) -> Result<rppal::gpio::InputPin> {
	let gpio = Gpio::new().into_diagnostic().wrap_err("gpio: init")?;
	let mut input = gpio
		.get(pin)
		.into_diagnostic()
		.wrap_err_with(|| format!("gpio: open BCM pin {pin}"))?
		.into_input_pullup();
	input
		.set_async_interrupt(Trigger::FallingEdge, Some(debounce), move |_event| {
			info!(pin, "gpio button press, authorizing");
			auth.authorize();
		})
		.into_diagnostic()
		.wrap_err_with(|| format!("gpio: register interrupt on BCM pin {pin}"))?;
	Ok(input)
}
