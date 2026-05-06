use std::time::{Duration, Instant};

use clap::Parser;
use improv_wifi::{
	AuthHandle, AuthorizeMode, Connection, ImprovWifi, ImprovWifiConfig, OwnedObjectPath,
	find_adapter, networkmanager::NetworkManagerBackend, power_on_adapter,
};
use miette::{IntoDiagnostic, Result, WrapErr};
use rppal::gpio::{Gpio, InputPin, Trigger};
use tokio::{
	io::{AsyncBufReadExt, BufReader},
	sync::{broadcast, mpsc},
};
use tracing::{debug, info, warn};

use crate::actions::{Context, iti::ItiArgs};

/// Run the Improv-Wi-Fi BLE peripheral so a phone or browser can provision the device's Wi-Fi.
///
/// Uses BlueZ for BLE and NetworkManager for Wi-Fi configuration.
///
/// Default mode is a long-running daemon that advertises only on demand (a fresh device
/// with no Wi-Fi config advertises immediately for first-boot provisioning; once
/// provisioned, the device stays idle until a long-press on `--auth-gpio` re-enters
/// provisioning mode). Use `--one-shot` for the legacy single-provisioning behaviour.
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

	/// Authorise when a line is received on stdin (the line content is ignored).
	///
	/// When set, the device starts in `AuthorizationRequired` and only accepts credentials
	/// after the first line on stdin. Only valid with `--one-shot`.
	#[arg(
		long,
		requires = "one_shot",
		conflicts_with_all = ["auth_gpio", "no_auth"],
	)]
	pub auth_stdin: bool,

	/// Authorise on a button press on this BCM GPIO pin.
	///
	/// The pin is configured as input with the internal pull-up resistor; wire a momentary
	/// switch from the pin to GND.
	///
	/// In default (daemon) mode this is the long-press trigger to enter provisioning mode and
	/// the short-press trigger to authorise an in-progress session. In `--one-shot` mode any
	/// press authorises the single session.
	#[arg(long, required_unless_present_any = ["auth_stdin", "no_auth"])]
	pub auth_gpio: Option<u8>,

	/// Debounce window for `--auth-gpio`.
	#[arg(long, default_value = "50ms")]
	pub auth_gpio_debounce: humantime::Duration,

	/// Hold time on `--auth-gpio` that counts as a long press (daemon mode only).
	#[arg(long, default_value = "3s")]
	pub auth_gpio_long_press: humantime::Duration,

	/// How long an authorisation stays valid before the device reverts to
	/// `AuthorizationRequired`. If unset, the device stays authorised until provisioned or
	/// shut down.
	#[arg(long)]
	pub auth_timeout: Option<humantime::Duration>,

	/// Skip authorisation gating: start the advertising session in `Authorized` and accept
	/// credentials from any device in BLE range without requiring a button press or stdin
	/// input.
	///
	/// SECURITY WARNING: this removes the physical-presence guarantee. Any device in BLE
	/// range during an advertising session can overwrite the device's Wi-Fi configuration.
	/// Requires `--one-shot`.
	#[arg(long, requires = "one_shot", conflicts_with = "auth_gpio")]
	pub no_auth: bool,

	/// Run even if Wi-Fi is already connected.
	///
	/// In `--one-shot` mode, the command exits cleanly when NetworkManager reports the Wi-Fi
	/// device is in the `Activated` state. Pass this flag to override that check.
	#[arg(long, requires = "one_shot")]
	pub always: bool,

	/// Run a single provisioning session and exit, instead of staying alive as a daemon.
	///
	/// SECURITY WARNING: while running, the device advertises over BLE until it is
	/// provisioned, expanding the BLE attack surface. The default daemon mode is invisible
	/// after provisioning and only re-enters advertising on a long-press of `--auth-gpio`.
	#[arg(long)]
	pub one_shot: bool,
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
	let device_name = args.device_name.clone().unwrap_or_else(|| hostname.clone());
	let local_name = args.local_name.clone().or(Some(hostname));

	let backend = NetworkManagerBackend::new(device_name)
		.await
		.into_diagnostic()
		.wrap_err("improv-wifi: connect to NetworkManager")?;

	let config = ImprovWifiConfig {
		authorize: if args.no_auth {
			AuthorizeMode::NotRequired
		} else {
			AuthorizeMode::Required
		},
		auth_timeout: args.auth_timeout.map(Duration::from),
		local_name,
	};

	if args.one_shot {
		run_one_shot(connection, adapter_path, backend, args, config).await
	} else {
		run_watch(connection, adapter_path, backend, args, config).await
	}
}

async fn run_one_shot(
	connection: Connection,
	adapter_path: OwnedObjectPath,
	backend: NetworkManagerBackend,
	args: ImprovWifiArgs,
	config: ImprovWifiConfig,
) -> Result<()> {
	if !args.always
		&& backend
			.is_connected()
			.await
			.into_diagnostic()
			.wrap_err("improv-wifi: query NetworkManager state")?
	{
		info!("Wi-Fi already connected; skipping Improv-Wi-Fi provisioning. Pass --always to override.");
		return Ok(());
	}

	info!("starting Improv-Wi-Fi service");
	let service = ImprovWifi::install(connection, adapter_path, backend, config)
		.await
		.into_diagnostic()
		.wrap_err("improv-wifi: install GATT service")?;

	let auth_handle = service.auth_handle();

	let stdin_task = args.auth_stdin.then(|| spawn_stdin_authoriser(auth_handle.clone()));

	let _gpio_pin = if let Some(pin) = args.auth_gpio {
		Some(
			install_short_press_authoriser(
				pin,
				args.auth_gpio_debounce.into(),
				auth_handle,
			)
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

async fn run_watch(
	connection: Connection,
	adapter_path: OwnedObjectPath,
	backend: NetworkManagerBackend,
	args: ImprovWifiArgs,
	config: ImprovWifiConfig,
) -> Result<()> {
	let pin_num = args
		.auth_gpio
		.expect("--watch requires --auth-gpio (clap should enforce)");

	let mut classifier = install_press_classifier(
		pin_num,
		args.auth_gpio_debounce.into(),
		args.auth_gpio_long_press.into(),
	)
	.wrap_err_with(|| format!("auth-gpio: configure BCM pin {pin_num}"))?;

	let configured = backend
		.is_configured()
		.await
		.into_diagnostic()
		.wrap_err("improv-wifi: query NetworkManager configured-connections")?;

	let mut should_advertise = !configured;

	if !configured {
		info!("no Wi-Fi configured; entering provisioning mode for initialisation");
	} else {
		info!(pin = pin_num, "Wi-Fi configured; idle until long-press");
	}

	loop {
		if should_advertise {
			run_advertising_session(
				connection.clone(),
				adapter_path.clone(),
				backend.clone(),
				config.clone(),
				classifier.short_press_tx.subscribe(),
			)
			.await?;
			info!("provisioning session ended; idle until next long-press");
		}

		match classifier.long_press_rx.recv().await {
			Some(()) => {
				info!("long press detected; entering provisioning mode");
				should_advertise = true;
			}
			None => {
				warn!("long-press channel closed; exiting watcher");
				break;
			}
		}
	}

	Ok(())
}

async fn run_advertising_session(
	connection: Connection,
	adapter_path: OwnedObjectPath,
	backend: NetworkManagerBackend,
	config: ImprovWifiConfig,
	mut short_press_rx: broadcast::Receiver<()>,
) -> Result<()> {
	info!("starting Improv-Wi-Fi advertising session");
	let service = ImprovWifi::install(connection, adapter_path, backend, config)
		.await
		.into_diagnostic()
		.wrap_err("improv-wifi: install GATT service")?;

	let auth_handle = service.auth_handle();
	let press_task = tokio::spawn(async move {
		loop {
			match short_press_rx.recv().await {
				Ok(()) => {
					info!("short press, authorising");
					auth_handle.authorize();
				}
				Err(broadcast::error::RecvError::Lagged(_)) => continue,
				Err(broadcast::error::RecvError::Closed) => break,
			}
		}
	});

	let result = service
		.run()
		.await
		.into_diagnostic()
		.wrap_err("improv-wifi: service loop");
	press_task.abort();
	result
}

fn read_hostname() -> String {
	std::fs::read_to_string("/etc/hostname")
		.ok()
		.map(|s| s.trim().to_owned())
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| "improv-device".into())
}

fn spawn_stdin_authoriser(auth: AuthHandle) -> tokio::task::JoinHandle<()> {
	tokio::spawn(async move {
		let mut lines = BufReader::new(tokio::io::stdin()).lines();
		loop {
			match lines.next_line().await {
				Ok(Some(_)) => {
					info!("stdin line received, authorising");
					auth.authorize();
				}
				Ok(None) => {
					debug!("stdin closed, ending stdin authoriser");
					break;
				}
				Err(err) => {
					warn!(?err, "stdin read error, ending stdin authoriser");
					break;
				}
			}
		}
	})
}

fn install_short_press_authoriser(
	pin: u8,
	debounce: Duration,
	auth: AuthHandle,
) -> Result<InputPin> {
	let gpio = Gpio::new().into_diagnostic().wrap_err("gpio: init")?;
	let mut input = gpio
		.get(pin)
		.into_diagnostic()
		.wrap_err_with(|| format!("gpio: open BCM pin {pin}"))?
		.into_input_pullup();
	input
		.set_async_interrupt(Trigger::FallingEdge, Some(debounce), move |_event| {
			info!(pin, "gpio button press, authorising");
			auth.authorize();
		})
		.into_diagnostic()
		.wrap_err_with(|| format!("gpio: register interrupt on BCM pin {pin}"))?;
	Ok(input)
}

/// Owns the GPIO pin and the classifier task; drop to release both.
struct PressClassifier {
	long_press_rx: mpsc::UnboundedReceiver<()>,
	short_press_tx: broadcast::Sender<()>,
	_pin: InputPin,
	_classifier_task: tokio::task::JoinHandle<()>,
}

#[derive(Debug, Clone, Copy)]
enum Edge {
	Down,
	Up,
}

fn install_press_classifier(
	pin: u8,
	debounce: Duration,
	long_press_window: Duration,
) -> Result<PressClassifier> {
	let gpio = Gpio::new().into_diagnostic().wrap_err("gpio: init")?;
	let mut input = gpio
		.get(pin)
		.into_diagnostic()
		.wrap_err_with(|| format!("gpio: open BCM pin {pin}"))?
		.into_input_pullup();

	let (edge_tx, mut edge_rx) = mpsc::unbounded_channel::<Edge>();
	let (long_press_tx, long_press_rx) = mpsc::unbounded_channel::<()>();
	let (short_press_tx, _) = broadcast::channel::<()>(8);

	input
		.set_async_interrupt(Trigger::Both, Some(debounce), move |event| {
			let kind = match event.trigger {
				Trigger::FallingEdge => Edge::Down,
				Trigger::RisingEdge => Edge::Up,
				_ => return,
			};
			let _ = edge_tx.send(kind);
		})
		.into_diagnostic()
		.wrap_err_with(|| format!("gpio: register interrupt on BCM pin {pin}"))?;

	let short_press_tx_clone = short_press_tx.clone();
	let classifier_task = tokio::spawn(async move {
		let mut press_start: Option<Instant> = None;
		while let Some(edge) = edge_rx.recv().await {
			match edge {
				Edge::Down => press_start = Some(Instant::now()),
				Edge::Up => {
					if let Some(start) = press_start.take() {
						let elapsed = start.elapsed();
						if elapsed >= long_press_window {
							debug!(?elapsed, "long press");
							let _ = long_press_tx.send(());
						} else {
							debug!(?elapsed, "short press");
							let _ = short_press_tx_clone.send(());
						}
					}
				}
			}
		}
	});

	Ok(PressClassifier {
		long_press_rx,
		short_press_tx,
		_pin: input,
		_classifier_task: classifier_task,
	})
}
