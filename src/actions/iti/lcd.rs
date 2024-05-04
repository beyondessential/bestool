use clap::Parser;
use miette::Result;

use crate::actions::Context;

mod io;

/// Control an LCD screen.
///
/// This is made for Waveshare's 1.69 inch LCD display, connected over SPI to a Raspberry Pi.
///
/// See more info about it here: https://www.waveshare.com/wiki/1.69inch_LCD_Module
///
/// You'll want to set up SPI's buffer size by adding `spidev.bufsiz=32768` to
/// `/boot/firmware/cmdline.txt`, otherwise you'll get "Message too long" errors.
#[derive(Debug, Clone, Parser)]
pub struct LcdArgs {
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
	#[arg(long, default_value = "10000000")]
	pub frequency: u32,
}

pub async fn run(ctx: Context<LcdArgs>) -> Result<()> {
	let mut lcd = io::LcdIo::new(&ctx.args_top)?;
	lcd.init()?;

	Ok(())
}
