use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

use clap::Parser;
use embedded_graphics::Drawable;
use miette::{IntoDiagnostic, Result, WrapErr};
use tracing::{error, info, trace};

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

	/// ZMQ REP socket to listen on for JSON screen updates.
	#[arg(default_value = "tcp://[::1]:2009")]
	pub zmq_socket: String,
}

pub async fn run(ctx: Context<LcdArgs>) -> Result<()> {
	let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    }).into_diagnostic().wrap_err("ctrlc: set_handler")?;

	let z = zmq::Context::new();
	let socket = z
		.socket(zmq::REQ)
		.into_diagnostic()
		.wrap_err("zmq: socket(REQ)")?;
	socket
		.set_ipv6(true)
		.into_diagnostic()
		.wrap_err("zmq: set_ipv6")?;
	socket
		.bind(&ctx.args_top.zmq_socket)
		.into_diagnostic()
		.wrap_err(format!("zmq: bind({})", ctx.args_top.zmq_socket))?;
	info!(
		"ZMQ REP listening on {} for JSON messages",
		ctx.args_top.zmq_socket
	);

	let mut lcd = io::LcdIo::new(&ctx.args_top)?;
	lcd.init()?;
	lcd.probe_buffer_length()?;

	loop {
		let mut polls = [socket.as_poll_item(zmq::POLLIN)];
		let polled = zmq::poll(&mut polls, 1000)
			.into_diagnostic()
			.wrap_err("zmq: poll")?;
		if running.load(Ordering::SeqCst) == false {
			info!("ctrl-c received, exiting");
			break;
		}
		if polled == 0 || !polls[0].is_readable() {
			trace!("zmq: no messages (poll timed out)");
			continue;
		}

		let Ok(bytes) = socket
			.recv_bytes(0)
			.map_err(|err| error!("zmq: failed to recv: {err}"))
		else {
			continue;
		};

		let Ok(screen @ json::Screen { .. }) =
			serde_json::from_slice(&bytes).map_err(|err| error!("json: failed to parse: {err}"))
		else {
			continue;
		};

		trace!(?screen, "received screen control message");

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
