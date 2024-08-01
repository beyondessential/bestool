use clap::{Parser, Subcommand};

use miette::Result;

use self::chip::Chip;
use super::ItiArgs;
use crate::actions::Context;

mod chip;
mod fill;
mod io;
mod lut;
mod pixels;
mod text;

/// Control an E-ink screen.
///
/// This is made for WeAct Studio's e-paper displays, connected over SPI to a Raspberry Pi.
///
/// You'll want to set up SPI's buffer size by adding `spidev.bufsiz=32768` to
/// `/boot/firmware/cmdline.txt`, otherwise you'll get "Message too long" errors.
#[derive(Debug, Clone, Parser)]
pub struct EinkArgs {
	/// Eink subcommand
	#[command(subcommand)]
	pub action: EinkAction,

	/// SPI port to use.
	#[arg(long, default_value = "0")]
	pub spi: u8,

	/// GPIO pin number for the display's busy pin.
	#[arg(long, default_value = "23")]
	pub busy: u8,

	/// GPIO pin number for the display's reset pin.
	#[arg(long, default_value = "24")]
	pub reset: u8,

	/// GPIO pin number for the display's data/command pin.
	#[arg(long, default_value = "25")]
	pub dc: u8,

	/// SPI CE number for the display's chip select pin.
	#[arg(long, default_value = "0")]
	pub ce: u8,

	/// SPI frequency in Hz.
	#[arg(long, default_value = "2000000")]
	pub frequency: u32,

	/// Display underlying chip.
	///
	/// The 1.54" (50mm) 200x200 display uses SSD1681, while the 2.13" (74mm) 176x296 displays use
	/// the SSD1680.
	#[arg(long, default_value = "SSD1681")]
	pub chip: Chip,

	/// Whether this is a three-colour display (two chromas).
	///
	/// This can be left off to use monochrome on a display capable of colour.
	#[arg(long)]
	pub bichromic: bool,

	/// Perform a partial update.
	///
	/// This is faster, but may leave ghosting and a grain pattern on the display. Monochrome only.
	#[arg(long, conflicts_with = "bichromic")]
	pub partial: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum EinkAction {
	Fill(fill::FillArgs),
	Text(text::TextArgs),
}

pub async fn run(ctx: Context<ItiArgs, EinkArgs>) -> Result<()> {
	match ctx.args_sub.action.clone() {
		EinkAction::Fill(subargs) => fill::run(ctx.push(subargs)).await,
		EinkAction::Text(subargs) => text::run(ctx.push(subargs)).await,
	}
}
