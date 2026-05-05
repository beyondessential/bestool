use clap::{Parser, Subcommand};
use miette::Result;

use crate::args::Args;

use super::Context;

pub mod samplers;

/// Tamanu Iti subcommands.
#[derive(Debug, Clone, Parser)]
pub struct ItiArgs {
	/// Subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[Context<Args, ItiArgs> => {|ctx: Context<Args, ItiArgs>| -> Result<(Action, Context<ItiArgs>)> {
		Ok((ctx.args_sub.action.clone(), ctx.push(())))
	}}](with_sub)

	#[cfg(feature = "iti-battery")]
	battery => Battery(BatteryArgs),
	#[cfg(feature = "iti-display")]
	display => Display(DisplayArgs),
	#[cfg(feature = "iti-temperature")]
	temperature => Temperature(TemperatureArgs)
}
