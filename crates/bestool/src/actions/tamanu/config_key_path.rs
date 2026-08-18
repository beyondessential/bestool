use std::path::{Path, PathBuf};

use clap::Parser;
use miette::{Result, bail};

use bestool_tamanu::{ApiServerKind, config::load_config, detect_kind};

use crate::actions::{
	Context,
	tamanu::{TamanuArgs, find_tamanu},
};

/// The config default when `crypto.keyFile` is unset, from
/// `packages/{central,facility}-server/config/default.json5` in Tamanu.
const DEFAULT_KEY_FILE: &str = "config/dev-secret.key";

/// Print the path to the Tamanu config key file.
///
/// The key encrypts every value in `local_system_secrets`: the settings PSK
/// (and so every secret setting), the device key, and a facility's sync
/// password. A database restored onto a host holding a different key reads none
/// of them, so a backup def names this command as its `path_command` to capture
/// the key with the database it belongs to.
///
/// Installs that mount the key from outside the filesystem — a container taking
/// it as a podman secret — have no path to print, and back up podman's secret
/// store instead.
#[derive(Debug, Clone, Parser)]
pub struct ConfigKeyPathArgs {
	/// Package to read the config for (central-server or facility-server).
	///
	/// Detected from the config when not given.
	#[arg(short, long)]
	pub package: Option<String>,
}

pub async fn run(args: ConfigKeyPathArgs, ctx: Context) -> Result<()> {
	let (_, root) = find_tamanu(ctx.require::<TamanuArgs>()).await?;
	let config = load_config(&root, args.package.as_deref())?;
	let kind = match args.package.as_deref().and_then(ApiServerKind::from_str_ci) {
		Some(kind) => kind,
		None => detect_kind(&config, None).await,
	};

	let key_file = config
		.crypto
		.as_ref()
		.and_then(|crypto| crypto.key_file.as_deref())
		.unwrap_or(DEFAULT_KEY_FILE);
	println!("{}", resolve_key_file(key_file, &root, kind)?.display());
	Ok(())
}

/// Resolve a relative key file the way the server does: against its working
/// directory, the server package directory under the install root. An absolute
/// path passes through. A relative path with no package directory to resolve
/// against is an error rather than a guess.
fn resolve_key_file(key_file: &str, root: &Path, kind: ApiServerKind) -> Result<PathBuf> {
	let path = Path::new(key_file);
	if path.is_absolute() {
		return Ok(path.to_path_buf());
	}
	let base = root.join("packages").join(kind.package_name());
	if !base.is_dir() {
		bail!(
			"crypto.keyFile is relative ({key_file}) and there is no {} to resolve it against; \
			 set it to an absolute path",
			base.display()
		);
	}
	Ok(base.join(path))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn absolute_key_file_passes_through() {
		let key = if cfg!(windows) {
			r"C:\Tamanu\config\tamanu.key"
		} else {
			"/run/secrets/tamanu-config-key"
		};
		assert_eq!(
			resolve_key_file(key, Path::new("/nonexistent"), ApiServerKind::Central).unwrap(),
			PathBuf::from(key)
		);
	}

	#[test]
	fn relative_key_file_resolves_against_the_package_dir() {
		let tmp = tempfile::tempdir().unwrap();
		let package_dir = tmp.path().join("packages").join("facility-server");
		std::fs::create_dir_all(&package_dir).unwrap();
		assert_eq!(
			resolve_key_file(DEFAULT_KEY_FILE, tmp.path(), ApiServerKind::Facility).unwrap(),
			package_dir.join(DEFAULT_KEY_FILE)
		);
		assert!(resolve_key_file(DEFAULT_KEY_FILE, tmp.path(), ApiServerKind::Central).is_err());
	}
}
