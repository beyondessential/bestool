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
	eink.init()?;

	match ctx.args_sub.colour {
		Colour::White => {
			let mut fill = Pixels::new_for(ctx.args_top.chip);
			fill.fill(true);
			if ctx.args_top.partial {
				eink.display_partial_monochrome(fill.as_reader())?;
			} else {
				eink.display_monochrome(fill.as_reader())?;
			}
		}
		Colour::Black => {
			let mut fill = Pixels::new_for(ctx.args_top.chip);
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

			let mut fill = Pixels::new_for(ctx.args_top.chip);
			fill.fill(true);
			eink.display_bichrome(
				Pixels::new_for(ctx.args_top.chip).as_reader(),
				fill.as_reader(),
			)?;
		}
		Colour::Bands => {
			let mut fill = Pixels::new_for(ctx.args_top.chip);
			fill.fill(false);

			let w = ctx.args_top.chip.width();
			let h = ctx.args_top.chip.height();

			for y in 0..h {
				for x in 0..w {
					fill.set(x, y, y % 10 < 5);
				}
			}

			if ctx.args_top.partial {
				eink.display_partial_monochrome(fill.as_reader())?;
			} else {
				eink.display_monochrome(fill.as_reader())?;
			}
		}
	}

	// eink.deep_sleep()?;
	Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum Colour {
	White,
	Black,
	Red,
	Bands,
}
