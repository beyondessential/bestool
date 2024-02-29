use clap::{Parser, Subcommand};

use miette::Result;

use super::Context;

mod fill;
mod io;
mod pixels;
mod lut;
mod text;

/// Control an E-ink screen.
///
/// This is made for WeAct Studio's e-paper displays, connected over SPI to a Raspberry Pi.
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

	/// Vertical resolution of the display.
	///
	/// The default is for the 1.54" (50mm) display.
	#[arg(long, default_value = "200")]
	pub height: u16,

	/// Horizontal resolution of the display.
	///
	/// The default is for the 1.54" (50mm) display.
	#[arg(long, default_value = "200")]
	pub width: u16,

	/// Whether this is a three-colour display (two chromas).
	///
	/// This can be left off to use monochrome on a display capable of colour.
	#[arg(long)]
	pub bichromic: bool,

	/// Perform a partial update.
	///
	/// This is faster, but may leave ghosting on the display. Monochrome only.
	#[arg(long, conflicts_with = "bichromic")]
	pub partial: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum EinkAction {
	Fill(fill::FillArgs),
	Text(text::TextArgs),
}

pub async fn run(ctx: Context<EinkArgs>) -> Result<()> {
	match ctx.args_top.action.clone() {
		EinkAction::Fill(subargs) => fill::run(ctx.with_sub(subargs)).await,
		EinkAction::Text(subargs) => text::run(ctx.with_sub(subargs)).await,
	}
}
