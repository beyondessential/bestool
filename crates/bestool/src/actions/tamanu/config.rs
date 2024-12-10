use std::{
	fs::File,
	io::Read,
	path::{Path, PathBuf},
};

use clap::Parser;
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use tracing::{debug, instrument};

use crate::actions::Context;

use super::{find_tamanu, TamanuArgs};

/// Find and print the current Tamanu config.
#[cfg_attr(docsrs, doc("\n\n**Command**: `bestool tamanu config`"))]
#[derive(Debug, Clone, Parser)]
pub struct ConfigArgs {
	/// Package to look at
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-p, --package central|facility`"))]
	#[arg(short, long)]
	pub package: String,

	/// Print compact JSON instead of pretty
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-c, --compact`"))]
	#[arg(short, long)]
	pub compact: bool,

	/// Print null if key not found
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-n, --or-null`"))]
	#[arg(short = 'n', long)]
	pub or_null: bool,

	/// Path to a subkey
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-k, --key`"))]
	#[arg(short, long)]
	pub key: Option<String>,

	/// If the value is a string, print it directly (without quotes)
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `-r, --raw`"))]
	#[arg(short, long)]
	pub raw: bool,
}

#[instrument(level = "debug")]
pub fn load_config(root: &Path, package: &str) -> Result<serde_json::Value> {
	let mut config = package_config(root, package, "default.json5")
		.transpose()?
		.unwrap_or_else(|| serde_json::Value::Object(Default::default()));

	if let Ok(env_name) = std::env::var("NODE_ENV") {
		if let Some(env_config) =
			package_config(root, package, &format!("{env_name}.json5")).transpose()?
		{
			config = merge_json(config, env_config);
		}
	} else {
		if let Some(env_config) = package_config(root, package, "production.json5").transpose()? {
			config = merge_json(config, env_config);
		}
	}

	if let Some(local_config) = package_config(root, package, "local.json5").transpose()? {
		config = merge_json(config, local_config);
	}

	Ok(config)
}

pub async fn run(ctx: Context<TamanuArgs, ConfigArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;

	let config = load_config(&root, &ctx.args_sub.package)?;

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

#[instrument(level = "debug")]
pub fn find_config_dir(root: &Path, package: &str, file: &str) -> Option<PathBuf> {
	// Windows installs
	let path = root
		.join("packages")
		.join(package)
		.join("config")
		.join(file);
	if path.exists() {
		return Some(path);
	}

	// Linux installs
	let path = root.join(file);
	if path.exists() {
		return Some(path);
	}

	None
}

#[instrument(level = "debug")]
pub fn package_config(root: &Path, package: &str, file: &str) -> Option<Result<serde_json::Value>> {
	fn inner(path: &Path) -> Result<serde_json::Value> {
		debug!(?path, "opening config file");
		let mut file = File::open(path).into_diagnostic()?;

		let mut contents = String::new();
		let bytes = file.read_to_string(&mut contents).into_diagnostic()?;
		debug!(%bytes, "read config file");

		let config: serde_json::Value = json5::from_str(&contents).into_diagnostic()?;
		Ok(config)
	}

	find_config_dir(root, package, file)
		.map(|path| inner(&path).wrap_err(path.to_string_lossy().into_owned()))
}

#[instrument(level = "trace")]
pub fn merge_json(
	mut base: serde_json::Value,
	mut overlay: serde_json::Value,
) -> serde_json::Value {
	if let (Some(base), Some(overlay)) = (base.as_object_mut(), overlay.as_object_mut()) {
		for (key, value) in overlay {
			if let Some(base_value) = base.get_mut(key) {
				*base_value = merge_json(base_value.clone(), value.clone());
			} else {
				base.insert(key.clone(), value.clone());
			}
		}
	} else {
		// If either or both of `base` and `overlay` are scalar values, it must be safe to simply overwrite the base.
		base = overlay
	}
	base
}
