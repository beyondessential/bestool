use clap::Parser;
use miette::{IntoDiagnostic, Result, bail};

use crate::actions::{
	Context,
	tamanu::{TamanuArgs, find_tamanu},
};

/// Find and print the current Tamanu config.
///
/// Alias: c
#[derive(Debug, Clone, Parser)]
pub struct ConfigArgs {
	/// Package to look at
	///
	/// If not provided, will look first for central then facility package.
	#[arg(short, long)]
	pub package: Option<String>,

	/// Print compact JSON instead of pretty
	#[arg(short, long)]
	pub compact: bool,

	/// Print null if key not found
	#[arg(short = 'n', long)]
	pub or_null: bool,

	/// Path to a subkey
	#[arg(short, long)]
	pub key: Option<String>,

	/// If the value is a string, print it directly (without quotes)
	#[arg(short, long)]
	pub raw: bool,
}

pub async fn run(ctx: Context<TamanuArgs, ConfigArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;

	let config = super::loader::load_config_as_object(&root, ctx.args_sub.package.as_deref())?;

	let value = if let Some(key) = &ctx.args_sub.key {
		let mut value = &config;
		for part in key.split('.') {
			value = match value.get(part) {
				Some(value) => value,
				None if ctx.args_sub.or_null => &serde_json::Value::Null,
				None => bail!("key not found: {:?}", key),
			};
		}
		value
	} else {
		&config
	};

	println!(
		"{}",
		match (ctx.args_sub, value.as_str()) {
			(ConfigArgs { raw: true, .. }, Some(string)) => {
				string.into()
			}
			(ConfigArgs { compact: true, .. }, _) => {
				serde_json::to_string(&value).into_diagnostic()?
			}
			_ => serde_json::to_string_pretty(&value).into_diagnostic()?,
		}
	);

	Ok(())
}
