use std::io::Read;

use clap::{Parser, Subcommand};
use embedded_graphics::Drawable;
use miette::{miette, IntoDiagnostic, Result, WrapErr};
use rpi_st7789v2_driver::{DriverArgs, Driver};
use tracing::{error, info, instrument, trace};
use zeromq::{Socket as _, SocketRecv as _, SocketSend as _, ZmqMessage};

use crate::actions::Context;

pub mod json;

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

impl From<LcdArgs> for DriverArgs {
	fn from(args: LcdArgs) -> Self {
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
	/// server. The command sends the message to the display server, then waits for a reply and
	/// prints it if non-empty.
	Send {
		/// JSON message to send.
		message: Option<String>,
	},

	/// Set all pixels to a single color.
	///
	/// The command sends the message to the display server, then waits for a reply and prints it
	/// if non-empty.
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
	/// The command sends the message to the display server, then waits for a reply and prints it
	/// if non-empty.
	On,

	/// Turn the display off.
	///
	/// This turns off the backlight and puts the display to sleep, which uses less power.
	///
	/// The LCD must then rest for 5ms before any further commands can be sent.
	///
	/// The command sends the message to the display server, then waits for a reply and prints it
	/// if non-empty.
	Off,
}

pub async fn run(args: LcdArgs, _ctx: Context) -> Result<()> {
	use LcdAction::*;
	match args.action.clone() {
		Serve => serve(args).await,
		Send { message } => {
			let screen = serde_json::from_str(&message.unwrap_or_else(|| {
				let mut buf = String::new();
				std::io::stdin().read_to_string(&mut buf).expect("stdin: ");
				buf
			}))
			.into_diagnostic()
			.wrap_err("json: from_str")?;
			send(&args.zmq_socket, screen).await
		}
		Clear { red, green, blue } => {
			send(&args.zmq_socket, json::Screen::Clear([red, green, blue])).await
		}
		On => send(&args.zmq_socket, json::Screen::Light(true)).await,
		Off => send(&args.zmq_socket, json::Screen::Light(false)).await,
	}
}

#[instrument(level = "debug", skip(args))]
pub async fn serve(args: LcdArgs) -> Result<()> {
	let mut socket = zeromq::RepSocket::new();
	socket
		.bind(&args.zmq_socket)
		.await
		.into_diagnostic()
		.wrap_err(format!("zmq: bind({})", args.zmq_socket))?;
	info!(
		"ZMQ REP listening on {} for JSON messages",
		args.zmq_socket
	);

	let mut lcd = Driver::new(args.into())?;
	lcd.init()?;
	lcd.probe_buffer_length()?;

	loop {
		tokio::select! {
			_ = tokio::signal::ctrl_c() => {
				info!("ctrl-c received, exiting");
				break;
			}
			request = socket.recv() => {
				let request = request.into_diagnostic().wrap_err("zmq: recv")?;
				// REP sockets must reply to every request: answer errors with
				// their text and successes with an empty message.
				let reply = match handle(request, &mut lcd) {
					Ok(()) => ZmqMessage::from(""),
					Err(err) => {
						let err = format!("{err:?}");
						error!("{err}");
						ZmqMessage::from(err)
					}
				};
				socket
					.send(reply)
					.await
					.into_diagnostic()
					.wrap_err("zmq: send")?;
			}
		}
	}

	Ok(())
}

#[instrument(level = "trace", skip(request, lcd))]
fn handle(request: ZmqMessage, lcd: &mut Driver) -> Result<()> {
	let bytes = request.get(0).ok_or_else(|| miette!("zmq: empty message"))?;

	let screen: json::Screen = serde_json::from_slice(bytes)
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
			info!("updating screen {otherwise:?}");
			otherwise.draw(lcd)?;
		}
	}

	Ok(())
}

#[instrument(level = "debug")]
pub async fn send(addr: &str, screen: json::Screen) -> Result<()> {
	let mut socket = zeromq::ReqSocket::new();
	socket
		.connect(addr)
		.await
		.into_diagnostic()
		.wrap_err(format!("zmq: connect({})", addr))?;

	let bytes = serde_json::to_vec(&screen)
		.into_diagnostic()
		.wrap_err("json: to_vec")?;
	socket
		.send(bytes.into())
		.await
		.into_diagnostic()
		.wrap_err("zmq: send")?;

	let reply = socket.recv().await.into_diagnostic().wrap_err("zmq: recv")?;
	let reply: String = reply
		.try_into()
		.map_err(|err| miette!("zmq: reply is not valid utf-8: {err}"))?;
	if !reply.is_empty() {
		println!("{reply}");
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn send_roundtrip() {
		let mut server = zeromq::RepSocket::new();
		let endpoint = server.bind("tcp://127.0.0.1:0").await.unwrap();

		let server_task = tokio::spawn(async move {
			let request = server.recv().await.unwrap();
			let screen: json::Screen = serde_json::from_slice(request.get(0).unwrap()).unwrap();
			assert!(matches!(screen, json::Screen::Light(true)));
			server.send(ZmqMessage::from("")).await.unwrap();
		});

		send(&endpoint.to_string(), json::Screen::Light(true))
			.await
			.unwrap();
		server_task.await.unwrap();
	}
}
