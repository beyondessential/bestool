use std::{
	io::Read,
	ops::ControlFlow,
	sync::{
		atomic::{AtomicBool, Ordering},
		Arc,
	},
};

use clap::{Parser, Subcommand};
use embedded_graphics::Drawable;
use miette::{miette, IntoDiagnostic, Result, WrapErr};
use tracing::{error, info, instrument, trace};

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

	/// ZMQ socket to use for JSON screen updates.
	#[arg(default_value = "tcp://[::1]:2009")]
	pub zmq_socket: String,

	/// Subcommand
	#[command(subcommand)]
	pub action: LcdAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum LcdAction {
	/// Start the LCD display server.
	///
	/// This will initiatialize the LCD display, listen for JSON messages on a ZMQ REP socket, and
	/// update the display based on the contents of the messages.
	///
	/// Note that enabling trace-level (`-vvv`) logging will considerably slow down screen updates,
	/// as it will log every command sent to the screen, which can be considerable for complex
	/// layouts and text.
	Serve,

	/// Send an arbitrary JSON message to the display server.
	///
	/// This is useful for debugging or testing the display server, or for interacting with the
	/// screen without a ZMQ client.
	///
	/// The message can be provided either as the first argument, or over stdin.
	///
	/// The message will be validated by the client to avoid sending malformed messages to the
	/// server. The command will block until the message can be sent to the display server, then
	/// wait for a reply and print it if non-empty.
	Send {
		/// JSON message to send.
		message: Option<String>,
	},

	/// Set all pixels to a single color.
	///
	/// The command will block until the message can be sent to the display server, then wait for a
	/// reply and print it if non-empty.
	Clear {
		/// Red value for the background color.
		#[arg(default_value = "0")]
		red: u8,

		/// Green value for the background color.
		#[arg(default_value = "0")]
		green: u8,

		/// Blue value for the background color.
		#[arg(default_value = "0")]
		blue: u8,
	},

	/// Turn the display on.
	///
	/// This wakes the display, turns on the backlight, and shows the current screen contents.
	///
	/// The LCD must then rest for 120ms before any further commands can be sent.
	///
	/// The command will block until the message can be sent to the display server, then wait for a
	/// reply and print it if non-empty.
	On,

	/// Turn the display off.
	///
	/// This turns off the backlight and puts the display to sleep, which uses less power.
	///
	/// The LCD must then rest for 5ms before any further commands can be sent.
	///
	/// The command will block until the message can be sent to the display server, then wait for a
	/// reply and print it if non-empty.
	Off,
}

pub async fn run(ctx: Context<LcdArgs>) -> Result<()> {
	use LcdAction::*;
	match ctx.args_top.action.clone() {
		Serve => serve(ctx),
		Send { message } => {
			let screen = serde_json::from_str(&message.unwrap_or_else(|| {
				let mut buf = String::new();
				std::io::stdin().read_to_string(&mut buf).expect("stdin: ");
				buf
			}))
			.into_diagnostic()
			.wrap_err("json: from_str")?;
			send(ctx, screen)
		}
		Clear { red, green, blue } => send(ctx, json::Screen::Clear([red, green, blue])),
		On => send(ctx, json::Screen::Light(true)),
		Off => send(ctx, json::Screen::Light(false)),
	}
}

#[instrument(level = "debug", skip(ctx))]
pub fn serve(ctx: Context<LcdArgs>) -> Result<()> {
	let running = Arc::new(AtomicBool::new(true));
	let r = running.clone();

	ctrlc::set_handler(move || {
		r.store(false, Ordering::SeqCst);
	})
	.into_diagnostic()
	.wrap_err("ctrlc: set_handler")?;

	let z = zmq::Context::new();
	let socket = z
		.socket(zmq::REP)
		.into_diagnostic()
		.wrap_err("zmq: socket(REP)")?;
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
		match loop_inner(running.clone(), &socket, &mut lcd) {
			Ok(ControlFlow::Continue(_)) => continue,
			Ok(ControlFlow::Break(_)) => break,
			Err(err) => {
				let err = format!("{err:?}");
				error!("{err}");
				socket.send(&err, 0).ok();
				continue;
			}
		}
	}

	Ok(())
}

#[instrument(level = "trace", skip(socket, lcd))]
fn loop_inner(
	running: Arc<AtomicBool>,
	socket: &zmq::Socket,
	lcd: &mut io::LcdIo,
) -> Result<ControlFlow<()>> {
	let mut polls = [socket.as_poll_item(zmq::POLLIN)];
	let polled = zmq::poll(&mut polls, 1000)
		.into_diagnostic()
		.wrap_err("zmq: poll")?;
	if running.load(Ordering::SeqCst) == false {
		info!("ctrl-c received, exiting");
		return Ok(ControlFlow::Break(()));
	}
	if polled == 0 || !polls[0].is_readable() {
		trace!("zmq: no messages (poll timed out)");
		return Ok(ControlFlow::Continue(()));
	}

	let bytes = socket
		.recv_bytes(0)
		.into_diagnostic()
		.wrap_err("zmq: recv")?;

	let screen: json::Screen = serde_json::from_slice(&bytes)
		.into_diagnostic()
		.wrap_err("json: parse")?;

	trace!(?screen, "received screen control message");

	use json::Screen::*;
	match screen {
		Light(true) => {
			info!("turning screen on");
			lcd.display(true)?;
			lcd.backlight(true);
			lcd.wake()?;
		}
		Light(false) => {
			info!("turning screen off");
			lcd.display(false)?;
			lcd.backlight(false);
			lcd.sleep()?;
		}
		otherwise => {
			info!("updating screen");
			otherwise.draw(lcd)?;
		}
	}

	socket
		.send(zmq::Message::new(), 0)
		.into_diagnostic()
		.wrap_err("zmq: send")?;

	Ok(ControlFlow::Continue(()))
}

#[instrument(level = "debug", skip(ctx))]
pub fn send(ctx: Context<LcdArgs>, screen: json::Screen) -> Result<()> {
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
		.connect(&ctx.args_top.zmq_socket)
		.into_diagnostic()
		.wrap_err(format!("zmq: connect({})", ctx.args_top.zmq_socket))?;

	let bytes = serde_json::to_vec(&screen)
		.into_diagnostic()
		.wrap_err("json: to_vec")?;
	socket
		.send(&bytes, 0)
		.into_diagnostic()
		.wrap_err("zmq: send")?;

	let reply = socket
		.recv_string(0)
		.into_diagnostic()
		.wrap_err("zmq: recv")?
		.map_err(|bytes| miette!("reply is not valid utf-8, received {} bytes", bytes.len()))
		.wrap_err("zmq: recv_string")?;
	if !reply.is_empty() {
		println!("{reply}");
	}

	Ok(())
}
