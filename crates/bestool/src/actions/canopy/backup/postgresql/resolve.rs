//! Resolve a postgres cluster's data directory from its `[postgresql]` config.
//!
//! Handles the standard Debian/Ubuntu layout
//! (`/var/lib/postgresql/<version>/<cluster>`) and the Windows installer layout
//! (`%ProgramFiles%\PostgreSQL\<version>\data`, which has no named clusters — the
//! configured `cluster` is then only a label), with explicit overrides
//! (`data_dir`, `version`) for anything non-standard. No Tamanu coupling.

use std::path::{Path, PathBuf};

use miette::{Result, bail};

use crate::actions::canopy::backup::method::PostgresqlConfig;

/// The base directory postgres data directories live under: `/var/lib/postgresql`
/// on Debian/Ubuntu, `%ProgramFiles%\PostgreSQL` on Windows.
#[cfg(not(windows))]
pub fn postgres_base() -> PathBuf {
	PathBuf::from("/var/lib/postgresql")
}

#[cfg(windows)]
pub fn postgres_base() -> PathBuf {
	std::env::var_os("ProgramFiles")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
		.join("PostgreSQL")
}

/// The data-directory leaf under `<base>/<version>/`. Debian names it after the
/// cluster; the Windows installer always uses `data` (it has no named clusters),
/// so the configured `cluster` is only a label there.
#[cfg(not(windows))]
fn cluster_subdir(config: &PostgresqlConfig) -> &str {
	&config.cluster
}

#[cfg(windows)]
fn cluster_subdir(_config: &PostgresqlConfig) -> &str {
	"data"
}

/// A resolved cluster: where its data directory is, and its version + name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCluster {
	pub data_dir: PathBuf,
	pub version: String,
	pub cluster: String,
}

/// Resolve the cluster against the standard base directory.
pub fn resolve(config: &PostgresqlConfig) -> Result<ResolvedCluster> {
	resolve_in(config, &postgres_base())
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
		let data_dir = base.join(version).join(cluster_subdir(config));
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
		let candidate = version_dir.join(cluster_subdir(config));
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

/// Resolve the *target* data directory for a restore.
///
/// Unlike [`resolve`], the directory need not exist yet (a fresh host): an
/// explicit `data_dir` or `version` fully determines the path. With neither, it
/// falls back to [`resolve`] (restoring over an already-present cluster).
pub fn resolve_target(config: &PostgresqlConfig) -> Result<ResolvedCluster> {
	resolve_target_in(config, &postgres_base())
}

fn resolve_target_in(config: &PostgresqlConfig, base: &Path) -> Result<ResolvedCluster> {
	if let Some(data_dir) = &config.data_dir {
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
	if let Some(version) = &config.version {
		return Ok(ResolvedCluster {
			version: version.clone(),
			cluster: config.cluster.clone(),
			data_dir: base.join(version).join(cluster_subdir(config)),
		});
	}
	resolve_in(config, base)
}

/// Find the data directory within a freshly-restored tree.
///
/// kopia restores the snapshot's source — for our backups that's the cluster
/// directory itself (`PG_VERSION` at the root). Older/looser layouts may nest it
/// under `<version>/<cluster>`, so search up to two levels deep.
pub fn locate_pgdata(staging: &Path) -> Result<PathBuf> {
	if is_data_dir(staging) {
		return Ok(staging.to_path_buf());
	}
	for depth1 in read_subdirs(staging) {
		if is_data_dir(&depth1) {
			return Ok(depth1);
		}
		for depth2 in read_subdirs(&depth1) {
			if is_data_dir(&depth2) {
				return Ok(depth2);
			}
		}
	}
	bail!(
		"no postgres data dir (PG_VERSION) found in restored tree {}",
		staging.display()
	)
}

fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
	let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
		.into_iter()
		.flatten()
		.flatten()
		.map(|e| e.path())
		.filter(|p| p.is_dir())
		.collect();
	out.sort();
	out
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
		let cfg = config("main", None, None);
		let leaf = cluster_subdir(&cfg);
		make_cluster(tmp.path(), "16", leaf);
		let resolved = resolve_in(&cfg, tmp.path()).unwrap();
		assert_eq!(resolved.version, "16");
		assert_eq!(resolved.cluster, "main");
		assert_eq!(resolved.data_dir, tmp.path().join("16").join(leaf));
	}

	#[test]
	fn version_pins_the_path() {
		let tmp = tempfile::tempdir().unwrap();
		let cfg = config("main", Some("15"), None);
		let leaf = cluster_subdir(&cfg);
		make_cluster(tmp.path(), "15", leaf);
		make_cluster(tmp.path(), "16", leaf);
		let resolved = resolve_in(&cfg, tmp.path()).unwrap();
		assert_eq!(resolved.version, "15");
		assert_eq!(resolved.data_dir, tmp.path().join("15").join(leaf));
	}

	#[test]
	fn ambiguous_without_version_errors() {
		let tmp = tempfile::tempdir().unwrap();
		let cfg = config("main", None, None);
		let leaf = cluster_subdir(&cfg);
		make_cluster(tmp.path(), "15", leaf);
		make_cluster(tmp.path(), "16", leaf);
		let err = resolve_in(&cfg, tmp.path()).unwrap_err();
		assert!(format!("{err}").contains("ambiguous"));
	}

	// On Debian the cluster name selects the directory; a wrong name finds nothing.
	#[cfg(not(windows))]
	#[test]
	fn missing_cluster_errors() {
		let tmp = tempfile::tempdir().unwrap();
		make_cluster(tmp.path(), "16", "main");
		let err = resolve_in(&config("other", None, None), tmp.path()).unwrap_err();
		assert!(format!("{err}").contains("no cluster"));
	}

	// On Windows there are no named clusters: any label resolves to `<version>\data`.
	#[cfg(windows)]
	#[test]
	fn windows_resolves_data_dir_regardless_of_cluster_label() {
		let tmp = tempfile::tempdir().unwrap();
		make_cluster(tmp.path(), "16", "data");
		let resolved = resolve_in(&config("any-label", None, None), tmp.path()).unwrap();
		assert_eq!(resolved.data_dir, tmp.path().join("16").join("data"));
		assert_eq!(resolved.cluster, "any-label");
	}

	#[test]
	fn resolve_target_allows_missing_dir_with_version() {
		let tmp = tempfile::tempdir().unwrap();
		let cfg = config("main", Some("16"), None);
		let leaf = cluster_subdir(&cfg);
		let resolved = resolve_target_in(&cfg, tmp.path()).unwrap();
		assert_eq!(resolved.version, "16");
		assert_eq!(resolved.data_dir, tmp.path().join("16").join(leaf));
		assert!(!resolved.data_dir.exists());
	}

	#[test]
	fn locate_pgdata_at_root_and_nested() {
		let tmp = tempfile::tempdir().unwrap();
		// Root.
		let root = tmp.path().join("root");
		std::fs::create_dir_all(&root).unwrap();
		std::fs::write(root.join("PG_VERSION"), "16").unwrap();
		assert_eq!(locate_pgdata(&root).unwrap(), root);

		// Nested <version>/<cluster>.
		let nested = tmp.path().join("nested");
		let inner = nested.join("16").join("main");
		std::fs::create_dir_all(&inner).unwrap();
		std::fs::write(inner.join("PG_VERSION"), "16").unwrap();
		assert_eq!(locate_pgdata(&nested).unwrap(), inner);

		// None.
		let empty = tmp.path().join("empty");
		std::fs::create_dir_all(&empty).unwrap();
		assert!(locate_pgdata(&empty).is_err());
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
