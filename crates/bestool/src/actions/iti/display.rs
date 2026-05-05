use std::{collections::HashSet, str::FromStr, time::Instant};

use clap::Parser;
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use miette::{IntoDiagnostic, Result, WrapErr};
use rpi_st7789v2_driver::{Driver, DriverArgs};
use tokio::signal::unix::{SignalKind, signal};
use tracing::{debug, info, instrument, warn};

use crate::actions::{Context, iti::ItiArgs};

mod canvas;
mod layout;
mod widget;
mod widgets;

pub use canvas::Canvas;
pub use layout::{LAYOUT, WidgetKind};
pub use widget::{DynWidget, Widget};

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

	/// Disable named widgets. Repeatable; comma-separated also accepted.
	///
	/// Valid widgets: clock, addresses, wifi, temperature, battery, sparks.
	#[arg(long, value_delimiter = ',', value_parser = clap::builder::ValueParser::new(WidgetKind::from_str))]
	pub disable: Vec<WidgetKind>,
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

pub async fn run(ctx: Context<ItiArgs, DisplayArgs>) -> Result<()> {
	serve(&ctx.args_sub).await
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

	let disabled: HashSet<WidgetKind> = args.disable.iter().copied().collect();
	let widgets = build_widgets(&disabled).await?;
	if widgets.is_empty() {
		warn!("no widgets enabled; the display will stay blank");
	}

	let mut term = signal(SignalKind::terminate())
		.into_diagnostic()
		.wrap_err("signal: SIGTERM")?;
	let mut int = signal(SignalKind::interrupt())
		.into_diagnostic()
		.wrap_err("signal: SIGINT")?;

	tokio::select! {
		res = tick_loop(widgets, &mut lcd) => res?,
		_ = term.recv() => info!("SIGTERM received, shutting down"),
		_ = int.recv() => info!("SIGINT received, shutting down"),
	}

	lcd.clear(Rgb565::BLACK)?;
	lcd.display(false)?;
	lcd.backlight(false);
	lcd.sleep()?;

	Ok(())
}

async fn build_widgets(disabled: &HashSet<WidgetKind>) -> Result<Vec<Box<dyn DynWidget>>> {
	let mut out: Vec<Box<dyn DynWidget>> = Vec::new();
	for entry in LAYOUT {
		if disabled.contains(&entry.kind) {
			info!(widget = entry.kind.name(), "disabled");
			continue;
		}
		match entry.kind {
			WidgetKind::Clock => {
				out.push(Box::new(widgets::clock::ClockWidget::new(entry.area)));
			}
			WidgetKind::Addresses => {
				out.push(Box::new(widgets::addresses::AddressesWidget::new(
					entry.area,
				)));
			}
			WidgetKind::Wifi => {
				out.push(Box::new(widgets::wifi::WifiWidget::new(entry.area)));
			}
			// Other widget kinds land in subsequent commits.
			_ => {}
		}
	}
	Ok(out)
}

async fn tick_loop(mut widgets: Vec<Box<dyn DynWidget>>, lcd: &mut Driver) -> Result<()> {
	if widgets.is_empty() {
		std::future::pending::<()>().await;
		unreachable!();
	}

	let now = Instant::now();
	let mut next_tick: Vec<Instant> = widgets.iter().map(|_| now).collect();

	loop {
		// Find the widget whose tick is due first, sleep until then, run it.
		let (idx, due) = next_tick
			.iter()
			.enumerate()
			.min_by_key(|(_, t)| *t)
			.map(|(i, t)| (i, *t))
			.expect("widgets is non-empty");

		let now = Instant::now();
		if due > now {
			tokio::time::sleep(due - now).await;
		}

		let interval = widgets[idx].interval();
		let name = widgets[idx].name();
		debug!(widget = name, "ticking");
		let mut canvas = Canvas::new(lcd);
		if let Err(err) = widgets[idx].tick(&mut canvas).await {
			warn!(widget = name, ?err, "widget tick failed");
		}
		next_tick[idx] = Instant::now() + interval;
	}
}
