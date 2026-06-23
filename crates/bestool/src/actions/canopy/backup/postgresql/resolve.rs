//! Resolve a postgres cluster's data directory from its `[postgresql]` config.
//!
//! Generic over the standard Debian/Ubuntu layout
//! (`/var/lib/postgresql/<version>/<cluster>`), with explicit overrides for
//! anything non-standard. No Tamanu coupling.

use std::path::{Path, PathBuf};

use miette::{Result, bail};

use crate::actions::canopy::backup::method::PostgresqlConfig;

/// The standard base directory clusters live under on Debian/Ubuntu.
pub const POSTGRES_BASE: &str = "/var/lib/postgresql";

/// A resolved cluster: where its data directory is, and its version + name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCluster {
	pub data_dir: PathBuf,
	pub version: String,
	pub cluster: String,
}

/// Resolve the cluster against the standard base directory.
pub fn resolve(config: &PostgresqlConfig) -> Result<ResolvedCluster> {
	resolve_in(config, Path::new(POSTGRES_BASE))
}

/// Resolve against a given base directory (the seam tests inject a temp tree at).
fn resolve_in(config: &PostgresqlConfig, base: &Path) -> Result<ResolvedCluster> {
	// An explicit data dir wins; derive version/cluster from it (or the config).
	if let Some(data_dir) = &config.data_dir {
		if !is_data_dir(data_dir) {
			bail!("{} is not a postgres data dir (no PG_VERSION)", data_dir.display());
		}
		let version = config
			.version
			.clone()
			.or_else(|| dir_name(data_dir.parent()))
			.unwrap_or_default();
		return Ok(ResolvedCluster {
			version,
			cluster: config.cluster.clone(),
			data_dir: data_dir.clone(),
		});
	}

	// With a version, the path is fully determined.
	if let Some(version) = &config.version {
		let data_dir = base.join(version).join(&config.cluster);
		if !is_data_dir(&data_dir) {
			bail!(
				"cluster '{}' version {} not found at {}",
				config.cluster,
				version,
				data_dir.display()
			);
		}
		return Ok(ResolvedCluster {
			version: version.clone(),
			cluster: config.cluster.clone(),
			data_dir,
		});
	}

	// Otherwise scan <base>/<version>/<cluster> for the one matching cluster.
	let mut matches: Vec<(String, PathBuf)> = Vec::new();
	let entries = std::fs::read_dir(base)
		.map_err(|e| miette::miette!("reading {}: {e}", base.display()))?;
	for entry in entries.flatten() {
		let version_dir = entry.path();
		let candidate = version_dir.join(&config.cluster);
		if is_data_dir(&candidate) {
			matches.push((
				dir_name(Some(&version_dir)).unwrap_or_default(),
				candidate,
			));
		}
	}
	matches.sort();
	match matches.len() {
		0 => bail!(
			"no cluster '{}' found under {}; set `version` or `data_dir`",
			config.cluster,
			base.display()
		),
		1 => {
			let (version, data_dir) = matches.into_iter().next().unwrap();
			Ok(ResolvedCluster {
				version,
				cluster: config.cluster.clone(),
				data_dir,
			})
		}
		_ => bail!(
			"cluster '{}' is ambiguous across versions {:?}; set `version`",
			config.cluster,
			matches.iter().map(|(v, _)| v).collect::<Vec<_>>()
		),
	}
}

fn is_data_dir(path: &Path) -> bool {
	path.join("PG_VERSION").is_file()
}

fn dir_name(path: Option<&Path>) -> Option<String> {
	path.and_then(|p| p.file_name())
		.map(|n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn config(cluster: &str, version: Option<&str>, data_dir: Option<PathBuf>) -> PostgresqlConfig {
		PostgresqlConfig {
			cluster: cluster.to_owned(),
			data_dir,
			version: version.map(str::to_owned),
			port: None,
			socket: None,
			strategy: None,
		}
	}

	fn make_cluster(base: &Path, version: &str, cluster: &str) {
		let dir = base.join(version).join(cluster);
		std::fs::create_dir_all(&dir).unwrap();
		std::fs::write(dir.join("PG_VERSION"), version).unwrap();
	}

	#[test]
	fn resolves_unique_cluster_by_scan() {
		let tmp = tempfile::tempdir().unwrap();
		make_cluster(tmp.path(), "16", "main");
		let resolved = resolve_in(&config("main", None, None), tmp.path()).unwrap();
		assert_eq!(resolved.version, "16");
		assert_eq!(resolved.cluster, "main");
		assert_eq!(resolved.data_dir, tmp.path().join("16").join("main"));
	}

	#[test]
	fn version_pins_the_path() {
		let tmp = tempfile::tempdir().unwrap();
		make_cluster(tmp.path(), "15", "main");
		make_cluster(tmp.path(), "16", "main");
		let resolved = resolve_in(&config("main", Some("15"), None), tmp.path()).unwrap();
		assert_eq!(resolved.version, "15");
		assert_eq!(resolved.data_dir, tmp.path().join("15").join("main"));
	}

	#[test]
	fn ambiguous_without_version_errors() {
		let tmp = tempfile::tempdir().unwrap();
		make_cluster(tmp.path(), "15", "main");
		make_cluster(tmp.path(), "16", "main");
		let err = resolve_in(&config("main", None, None), tmp.path()).unwrap_err();
		assert!(format!("{err}").contains("ambiguous"));
	}

	#[test]
	fn missing_cluster_errors() {
		let tmp = tempfile::tempdir().unwrap();
		make_cluster(tmp.path(), "16", "main");
		let err = resolve_in(&config("other", None, None), tmp.path()).unwrap_err();
		assert!(format!("{err}").contains("no cluster"));
	}

	#[test]
	fn explicit_data_dir_derives_version() {
		let tmp = tempfile::tempdir().unwrap();
		make_cluster(tmp.path(), "16", "main");
		let data_dir = tmp.path().join("16").join("main");
		let resolved =
			resolve_in(&config("main", None, Some(data_dir.clone())), tmp.path()).unwrap();
		assert_eq!(resolved.version, "16");
		assert_eq!(resolved.data_dir, data_dir);
	}
}
