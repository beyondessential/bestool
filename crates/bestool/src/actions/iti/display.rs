use clap::{Parser, Subcommand};
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use miette::{IntoDiagnostic, Result, WrapErr};
use rpi_st7789v2_driver::{Driver, DriverArgs};
use tokio::signal::unix::{SignalKind, signal};
use tracing::{info, instrument};

use crate::actions::{Context, iti::ItiArgs};

/// Drive the Iti's LCD with a fixed widget layout.
///
/// This is a single long-running service that owns the SPI/GPIO link to the panel and renders
/// every widget itself. There's no IPC: each widget samples whatever it needs (sensors, D-Bus,
/// etc.) on its own cadence.
///
/// You'll want to set `spidev.bufsiz=131072` in `/boot/firmware/cmdline.txt`, otherwise you'll
/// get "Message too long" errors.
#[derive(Debug, Clone, Parser)]
pub struct DisplayArgs {
	/// SPI port to use.
	#[arg(long, default_value = "0")]
	pub spi: u8,

	/// GPIO pin number for the display's backlight control pin.
	#[arg(long, default_value = "18")]
	pub backlight: u8,

	/// GPIO pin number for the display's reset pin.
	#[arg(long, default_value = "27")]
	pub reset: u8,

	/// GPIO pin number for the display's data/command pin.
	#[arg(long, default_value = "25")]
	pub dc: u8,

	/// SPI CE number for the display's chip select pin.
	#[arg(long, default_value = "0")]
	pub ce: u8,

	/// SPI frequency in Hz.
	#[arg(long, default_value = "20000000")]
	pub frequency: u32,

	/// Subcommand
	#[command(subcommand)]
	pub action: DisplayAction,
}

impl From<&DisplayArgs> for DriverArgs {
	fn from(args: &DisplayArgs) -> Self {
		DriverArgs {
			spi: args.spi,
			backlight: args.backlight,
			reset: args.reset,
			dc: args.dc,
			ce: args.ce,
			frequency: args.frequency,
		}
	}
}

#[derive(Debug, Clone, Subcommand)]
pub enum DisplayAction {
	/// Run the display service in the foreground.
	Run,
}

pub async fn run(ctx: Context<ItiArgs, DisplayArgs>) -> Result<()> {
	match ctx.args_sub.action {
		DisplayAction::Run => serve(&ctx.args_sub).await,
	}
}

#[instrument(level = "debug", skip(args))]
async fn serve(args: &DisplayArgs) -> Result<()> {
	let mut lcd = Driver::new(args.into())?;
	lcd.init()?;
	lcd.probe_buffer_length()?;
	lcd.clear(Rgb565::BLACK)?;
	lcd.display(true)?;
	lcd.backlight(true);
	lcd.wake()?;

	info!("display ready, awaiting SIGTERM/SIGINT");

	let mut term = signal(SignalKind::terminate())
		.into_diagnostic()
		.wrap_err("signal: SIGTERM")?;
	let mut int = signal(SignalKind::interrupt())
		.into_diagnostic()
		.wrap_err("signal: SIGINT")?;

	tokio::select! {
		_ = term.recv() => info!("SIGTERM received, shutting down"),
		_ = int.recv() => info!("SIGINT received, shutting down"),
	}

	lcd.clear(Rgb565::BLACK)?;
	lcd.display(false)?;
	lcd.backlight(false);
	lcd.sleep()?;

	Ok(())
}
