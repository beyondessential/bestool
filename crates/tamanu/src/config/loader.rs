use std::{
	fs::File,
	io::Read,
	path::{Path, PathBuf},
};

use miette::{IntoDiagnostic, Result, WrapErr};
use tracing::{debug, instrument};

#[instrument(level = "debug")]
pub fn load_config(root: &Path, package: Option<&str>) -> Result<super::structure::TamanuConfig> {
	serde_json::from_value(load_config_as_object(root, package)?).into_diagnostic()
}

#[instrument(level = "debug")]
pub fn load_config_as_object(root: &Path, package: Option<&str>) -> Result<serde_json::Value> {
	let mut config = package_config(root, package, "default.json5")
		.transpose()?
		.unwrap_or_else(|| serde_json::Value::Object(Default::default()));

	if let Some(env_config) = package_config(root, package, &env_config_file()).transpose()? {
		config = merge_json(config, env_config);
	}

	if let Some(local_config) = package_config(root, package, "local.json5").transpose()? {
		config = merge_json(config, local_config);
	}

	Ok(config)
}

pub fn env_config_file() -> String {
	let env_name = std::env::var("NODE_ENV").unwrap_or_else(|_| "production".into());
	format!("{env_name}.json5")
}

#[instrument(level = "debug")]
pub fn find_config_dir(root: &Path, package: Option<&str>, file: &str) -> Option<PathBuf> {
	// Windows installs
	if let Some(package) = package {
		let path = root
			.join("packages")
			.join(package)
			.join("config")
			.join(file);
		if path.exists() {
			return Some(path);
		}
	} else {
		for package in ["central-server", "facility-server"] {
			let path = root
				.join("packages")
				.join(package)
				.join("config")
				.join(file);
			if path.exists() {
				return Some(path);
			}
		}
	}

	// Linux installs
	let path = root.join(file);
	if path.exists() {
		return Some(path);
	}

	None
}

fn read_config_file(path: &Path) -> Result<serde_json::Value> {
	debug!(?path, "opening config file");
	let mut file = File::open(path).into_diagnostic()?;

	let mut contents = String::new();
	let bytes = file.read_to_string(&mut contents).into_diagnostic()?;
	debug!(%bytes, "read config file");

	let config: serde_json::Value = json5::from_str(&contents).into_diagnostic()?;
	Ok(config)
}

/// A config file as it sits on this host's disk, never one bundled in a container
/// image.
#[instrument(level = "debug")]
pub fn package_config_on_disk(
	root: &Path,
	package: Option<&str>,
	file: &str,
) -> Option<Result<serde_json::Value>> {
	find_config_dir(root, package, file)
		.map(|path| read_config_file(&path).wrap_err(path.to_string_lossy().into_owned()))
}

#[instrument(level = "debug")]
pub fn package_config(
	root: &Path,
	package: Option<&str>,
	file: &str,
) -> Option<Result<serde_json::Value>> {
	if let Some(result) = package_config_on_disk(root, package, file) {
		return Some(result);
	}

	// Linux container deployments only ship operator overrides on disk; the
	// in-image base configs (notably `default.json5`) sit at
	// `/app/packages/{package}/config/<file>` inside the running container.
	// Without this fallback, downstream consumers see partial config.
	if super::container::root_looks_like_container_host(root)
		&& let Some(result) = super::container::read_bundled_config(package, file)
	{
		return Some(result.wrap_err_with(|| format!("extracting {file} from container")));
	}

	None
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
