use clap::Parser;
use embedded_graphics::Drawable;
use miette::Result;

use crate::actions::Context;

mod commands;
mod helpers;
mod io;
mod json;
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
	#[arg(long, default_value = "20000000")]
	pub frequency: u32,
}

pub async fn run(ctx: Context<LcdArgs>) -> Result<()> {
	let lines = std::io::stdin().lines();

	let mut lcd = io::LcdIo::new(&ctx.args_top)?;
	lcd.init()?;
	lcd.probe_buffer_length()?;

	for line in lines {
		let screen: json::Screen = match line
			.map_err(|err| err.to_string())
			.and_then(|line| serde_json::from_str(&line).map_err(|err| err.to_string()))
		{
			Ok(screen) => screen,
			Err(err) => {
				eprintln!("error parsing JSON line: {err}");
				continue;
			}
		};

		if !screen.off {
			lcd.display(true)?;
			lcd.backlight(true);
			lcd.wake()?;
		}

		screen.draw(&mut lcd)?;

		if screen.off {
			lcd.display(false)?;
			lcd.backlight(false);
			lcd.sleep()?;
		}
	}

	Ok(())
}
