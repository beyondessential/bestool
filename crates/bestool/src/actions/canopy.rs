use std::io::Read as _;

use base64::{
	Engine as _,
	engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use clap::{Parser, Subcommand};
use miette::{IntoDiagnostic as _, Result, bail, miette};

use super::Context;

/// Interact with Canopy (the Tamanu meta-monitoring service).
#[derive(Debug, Clone, Parser)]
pub struct CanopyArgs {
	/// Canopy subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[CanopyArgs => |args: CanopyArgs, mut ctx: Context| -> Result<(Action, Context)> {
		let action = args.action.clone();
		ctx.provide(args);
		Ok((action, ctx))
	}]

	#[cfg(feature = "canopy-register")]
	register => Register(RegisterArgs),
	#[cfg(feature = "canopy-export")]
	export => Export(ExportArgs),
	#[cfg(feature = "canopy-import")]
	import => Import(ImportArgs)
}

/// Load the registration for a command that takes an optional `--config <DIR>`.
///
/// With an explicit dir, reads exactly that dir. With the default location,
/// uses the migration-aware loader so a legacy host that the daemon hasn't
/// migrated yet is still picked up.
#[cfg(any(feature = "canopy-register", feature = "canopy-export"))]
async fn load_registration(
	config: Option<&std::path::Path>,
) -> Result<Option<bestool_canopy::registration::Registration>> {
	match config {
		Some(dir) => bestool_canopy::registration::load_from(dir).await,
		None => bestool_canopy::registration::load().await,
	}
}

/// Read base64 input from stdin, erroring if it's empty.
#[cfg(any(feature = "canopy-register", feature = "canopy-import"))]
fn read_stdin(what: &str) -> Result<String> {
	let mut buf = String::new();
	std::io::stdin()
		.read_to_string(&mut buf)
		.into_diagnostic()
		.map_err(|e| miette!("reading {what} from stdin: {e}"))?;
	if buf.trim().is_empty() {
		bail!("no {what} given on the command line or stdin");
	}
	Ok(buf)
}

/// Base64-decode input, accepting every variant Canopy's lenient encoder might
/// produce (standard / no-pad / url-safe / url-safe-no-pad).
#[cfg(any(feature = "canopy-register", feature = "canopy-import"))]
fn decode_base64(input: &str) -> Result<Vec<u8>> {
	for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
		if let Ok(bytes) = engine.decode(input) {
			return Ok(bytes);
		}
	}
	Err(miette!("input is not valid base64"))
}

#[cfg(test)]
#[cfg(any(feature = "canopy-register", feature = "canopy-import"))]
mod tests {
	use super::*;

	#[test]
	fn decode_base64_accepts_all_variants() {
		let raw = b"\x00\xff\x10hello world?!";
		for encoded in [
			STANDARD.encode(raw),
			STANDARD_NO_PAD.encode(raw),
			URL_SAFE.encode(raw),
			URL_SAFE_NO_PAD.encode(raw),
		] {
			assert_eq!(decode_base64(&encoded).unwrap(), raw);
		}
	}

	#[test]
	fn decode_base64_rejects_garbage() {
		assert!(decode_base64("not valid base64 !!!! \u{00a0}").is_err());
	}
}
