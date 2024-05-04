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
	}}]

	#[cfg(feature = "iti-eink")]
	eink => Eink(EinkArgs),
	#[cfg(feature = "iti-wifisetup")]
	wifisetup => WifiSetup(WifisetupArgs)
}
