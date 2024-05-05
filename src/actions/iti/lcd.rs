use clap::Parser;
use embedded_graphics::pixelcolor::Rgb565;
use miette::Result;

use crate::actions::Context;

mod commands;
mod helpers;
mod io;
mod simple;

/// Control an LCD screen.
///
/// This is made for Waveshare's 1.69 inch LCD display, connected over SPI to a Raspberry Pi.
///
/// See more info about it here: https://www.waveshare.com/wiki/1.69inch_LCD_Module
///
/// You'll want to set up SPI's buffer size by adding `spidev.bufsiz=131072` to
/// `/boot/firmware/cmdline.txt`, otherwise you'll get "Message too long" errors.
// 131072 = closest power of 2 to 128400, which is size of the display's framebuffer.
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

	/// Red channel for the solid color.
	pub red: u8,

	/// Green channel for the solid color.
	pub green: u8,

	/// Blue channel for the solid color.
	pub blue: u8,
}

pub async fn run(ctx: Context<LcdArgs>) -> Result<()> {
	let LcdArgs { red, green, blue, .. } = ctx.args_top;
	let mut lcd = io::LcdIo::new(&ctx.args_top)?;
	lcd.init()?;

	let mut image = lcd.image();
	image.solid(Rgb565::new(red, green, blue));
	lcd.print(&image)?;

	Ok(())
}
