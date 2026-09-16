//! bliti: QR-anchored BLE device provisioning.
//!
//! Two things live in this binary: the daemon a device runs, and the sticker generator. They share
//! the board-ID precedence and the key schedule in `bliti-core`, which is what makes the sticker a
//! generator prints match the handle the device advertises.
//!
//! Behaviour is specified under `.workhorse/specs/bliti/`.

use std::{path::PathBuf, time::Duration};

use clap::{Parser, Subcommand};
use miette::{IntoDiagnostic, Result, WrapErr};

mod facts;
mod gatt;
mod identity;
mod session;
mod sticker;

#[cfg(target_os = "linux")]
mod client;
#[cfg(target_os = "linux")]
mod device;

/// How often the rotation salt changes (BLI-ADV, "Rotation"). A client recomputes against whatever
/// salt it observes, so nothing a client does depends on this period.
pub const SALT_ROTATION: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Parser)]
#[command(name = "bliti", version, about = "QR-anchored BLE device provisioning")]
struct Cli {
	#[command(subcommand)]
	command: Command,

	/// Where the derived sticker secret is cached.
	#[arg(long, global = true, default_value_os_t = identity::default_cache_path())]
	cache: PathBuf,
}

#[derive(Debug, Subcommand)]
enum Command {
	/// Advertise over BLE and serve provisioning sessions.
	Daemon {
		/// Bluetooth adapter to use. Defaults to the system's first.
		#[arg(long)]
		adapter: Option<String>,
	},

	/// Print the sticker for the board this runs on.
	Sticker {
		/// Write the QR code as SVG rather than drawing it in the terminal.
		#[arg(long)]
		svg: bool,
	},

	/// Report which board ID source this board offers and which one wins, without deriving anything.
	BoardId,

	/// Scan for the device a sticker belongs to. The client half of discovery, without a browser.
	Scan {
		/// The sticker payload: a sticker URL, its fragment, or the rendering printed beneath the code.
		sticker: String,

		/// How long to listen for.
		#[arg(long, default_value_t = 10)]
		seconds: u64,

		/// Bluetooth adapter to use. Defaults to the system's first.
		#[arg(long)]
		adapter: Option<String>,
	},

	/// Open a channel to a device and exchange the milestone's two messages.
	Connect {
		/// The sticker payload: a sticker URL, its fragment, or the rendering printed beneath the code.
		sticker: String,

		/// The device's address, as reported by `scan`. Found by matching the sticker when absent.
		#[arg(long)]
		address: Option<String>,

		/// A line of text for the device to print.
		#[arg(long, default_value = "hello from the command line")]
		text: String,

		/// Bluetooth adapter to use. Defaults to the system's first.
		#[arg(long)]
		adapter: Option<String>,
	},
}

fn main() -> Result<()> {
	tracing_subscriber::fmt()
		.with_env_filter(
			tracing_subscriber::EnvFilter::try_from_env("BLITI_LOG")
				.unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
		)
		.with_writer(std::io::stderr)
		.init();

	let cli = Cli::parse();
	let runtime = tokio::runtime::Runtime::new().into_diagnostic()?;
	runtime.block_on(run(cli))
}

async fn run(cli: Cli) -> Result<()> {
	match cli.command {
		Command::BoardId => board_id(),
		Command::Sticker { svg } => make_sticker(&cli.cache, svg),
		Command::Daemon { adapter } => daemon(&cli.cache, adapter.as_deref()).await,
		Command::Scan {
			sticker,
			seconds,
			adapter,
		} => scan(&sticker, seconds, adapter.as_deref()).await,
		Command::Connect {
			sticker,
			address,
			text,
			adapter,
		} => connect(&sticker, address.as_deref(), &text, adapter.as_deref()).await,
	}
}

/// Report what the board offers. Probes only: no source value is read, so this is safe and instant
/// even where the winning source is a TPM.
fn board_id() -> Result<()> {
	use bliti_core::board_id::{BoardIdSource, strongest_present};

	identity::guard_unreadable_sources().into_diagnostic()?;
	let sources = identity::sources();
	for source in &sources {
		let presence = source.probe().into_diagnostic()?;
		println!("{:>28}  {presence:?}", source.kind().to_string());
	}

	let refs: Vec<&dyn BoardIdSource> = sources.iter().map(AsRef::as_ref).collect();
	match strongest_present(&refs).into_diagnostic()? {
		Some(kind) => println!("\nwinning source: {kind}"),
		None => println!("\nno usable source: this board cannot derive a sticker"),
	}
	Ok(())
}

/// Print the sticker for this board, deriving its secret if the cache does not already hold it.
fn make_sticker(cache: &std::path::Path, svg: bool) -> Result<()> {
	let identity = identity::establish(cache)
		.into_diagnostic()
		.wrap_err("establishing this board's identity")?;
	if identity.derived {
		tracing::info!(source = %identity.kind, "derived this board's sticker secret");
	}

	let payload = bliti_core::sticker::StickerPayload::new(identity.secret);
	let sticker = sticker::Sticker::new(&payload).into_diagnostic()?;

	if svg {
		println!("{}", sticker.to_svg());
	} else {
		println!("{}", sticker.to_terminal());
		println!("{}", sticker.url);
	}
	// The human-readable rendering is printed beneath the code, so a scuffed sticker stays usable.
	println!("\n{}", sticker.human);
	Ok(())
}

#[cfg(target_os = "linux")]
async fn daemon(cache: &std::path::Path, adapter: Option<&str>) -> Result<()> {
	device::run(cache, adapter).await
}

/// Read a sticker however it was given: the URL a code encodes, its fragment alone, or the
/// human-readable rendering printed beneath the code. All three carry the same payload.
fn read_sticker(given: &str) -> Result<bliti_core::sticker::StickerPayload> {
	bliti_core::sticker::StickerPayload::read(given)
		.into_diagnostic()
		.wrap_err("reading the sticker")
}

#[cfg(target_os = "linux")]
async fn scan(sticker: &str, seconds: u64, adapter: Option<&str>) -> Result<()> {
	device::scan(&read_sticker(sticker)?, seconds, adapter).await
}

#[cfg(target_os = "linux")]
async fn connect(
	sticker: &str,
	address: Option<&str>,
	text: &str,
	adapter: Option<&str>,
) -> Result<()> {
	let payload = read_sticker(sticker)?;
	let address = address
		.map(str::parse::<bluer::Address>)
		.transpose()
		.into_diagnostic()
		.wrap_err("reading the device address")?;
	client::connect(address, payload.secret(), text, adapter).await
}

#[cfg(not(target_os = "linux"))]
async fn connect(_s: &str, _a: Option<&str>, _t: &str, _ad: Option<&str>) -> Result<()> {
	miette::bail!("connecting runs on Linux, against BlueZ")
}

#[cfg(not(target_os = "linux"))]
async fn scan(_sticker: &str, _seconds: u64, _adapter: Option<&str>) -> Result<()> {
	miette::bail!("scanning runs on Linux, against BlueZ")
}

#[cfg(not(target_os = "linux"))]
async fn daemon(_cache: &std::path::Path, _adapter: Option<&str>) -> Result<()> {
	miette::bail!("the bliti daemon runs on Linux, against BlueZ")
}
