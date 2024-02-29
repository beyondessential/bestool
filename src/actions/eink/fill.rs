use clap::{Parser, ValueEnum};
use miette::{bail, Result};

use crate::actions::Context;

use super::{io::EinkIo, pixels::Pixels, EinkArgs};

/// Fill the screen with one colour.
#[derive(Debug, Clone, Parser)]
pub struct FillArgs {
	/// Colour to use
	#[arg(alias = "color")]
	pub colour: Colour,
}

pub async fn run(ctx: Context<EinkArgs, FillArgs>) -> Result<()> {
	let mut eink = EinkIo::new(&ctx.args_top)?;
	eink.wake();

	match ctx.args_sub.colour {
		Colour::White => {
			let mut fill = Pixels::new_for(&eink);
			fill.fill(true);
			if ctx.args_top.partial {
				eink.display_partial_monochrome(fill.as_reader())?;
			} else {
				eink.display_monochrome(fill.as_reader())?;
			}
		}
		Colour::Black => {
			let mut fill = Pixels::new_for(&eink);
			fill.fill(false);
			if ctx.args_top.partial {
				eink.display_partial_monochrome(fill.as_reader())?;
			} else {
				eink.display_monochrome(fill.as_reader())?;
			}
		}
		Colour::Red => {
			if !ctx.args_top.bichromic {
				bail!("Red is only available with --bichromic");
			}

			let mut fill = Pixels::new_for(&eink);
			fill.fill(true);
			eink.display_bichrome(Pixels::new_for(&eink).as_reader(), fill.as_reader())?;
		}
	}

	eink.deep_sleep()?;
	Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum Colour {
	White,
	Black,
	Red,
}
