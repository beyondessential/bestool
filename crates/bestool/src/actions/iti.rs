use std::time::Duration;

use clap::{Parser, Subcommand};
use miette::Result;

use crate::args::Args;

use super::Context;

pub(crate) fn parse_friendly_duration(s: &str) -> Result<Duration, String> {
	let signed: jiff::SignedDuration = s.parse().map_err(|e: jiff::Error| e.to_string())?;
	signed.try_into().map_err(|e: jiff::Error| e.to_string())
}

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
	#[cfg(feature = "iti-improv-wifi")]
	improv_wifi => ImprovWifi(ImprovWifiArgs),
	#[cfg(feature = "iti-lcd")]
	lcd => Lcd(LcdArgs),
	#[cfg(feature = "iti-lcd")]
	sparks => Sparks(SparksArgs),
	#[cfg(feature = "iti-temperature")]
	temperature => Temperature(TemperatureArgs)
}
