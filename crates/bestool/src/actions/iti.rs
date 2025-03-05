use clap::{Parser, Subcommand};
use miette::Result;

use super::Context;

/// Tamanu Iti subcommands.
#[derive(Debug, Clone, Parser)]
pub struct ItiArgs {
	/// Subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[Context<ItiArgs> => {|ctx: Context<ItiArgs>| -> Result<(Action, Context<ItiArgs>)> {
		Ok((ctx.args_top.action.clone(), ctx.with_sub(())))
	}}](with_sub)

	#[cfg(feature = "iti-battery")]
	battery => Battery(BatteryArgs),
	#[cfg(feature = "iti-lcd")]
	lcd => Lcd(LcdArgs),
	#[cfg(feature = "iti-lcd")]
	sparks => Sparks(SparksArgs),
	#[cfg(feature = "iti-temperature")]
	temperature => Temperature(TemperatureArgs)
}
