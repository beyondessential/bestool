use clap::Parser;
use miette::Result;
use tokio::time::sleep;

use crate::actions::{
	Context,
	iti::{ItiArgs, samplers::temperature::sample},
};

/// Get core temperature from the Raspberry Pi.
#[derive(Debug, Clone, Parser)]
pub struct TemperatureArgs {
	/// Output in JSON format.
	#[arg(long)]
	pub json: bool,

	/// Keep updating at an interval.
	///
	/// Syntax is a number followed by a unit, such as "5s" or "1m".
	#[arg(long)]
	pub watch: Option<humantime::Duration>,
}

pub async fn run(ctx: Context<ItiArgs, TemperatureArgs>) -> Result<()> {
	if let Some(n) = ctx.args_sub.watch {
		loop {
			once(&ctx.args_sub)?;
			sleep(*n).await;
		}
	} else {
		once(&ctx.args_sub)
	}
}

fn once(args: &TemperatureArgs) -> Result<()> {
	let temperature = sample()?;
	if args.json {
		println!("{}", serde_json::json!({ "temperature": temperature }));
	} else {
		println!("{:.1}°C", temperature);
	}
	Ok(())
}
