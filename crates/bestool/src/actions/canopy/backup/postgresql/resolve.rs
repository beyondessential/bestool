//! Resolve a postgres cluster's data directory from its `[postgresql]` config.
//!
//! Handles the standard Debian/Ubuntu layout
//! (`/var/lib/postgresql/<version>/<cluster>`) and the Windows installer layout
//! (`%ProgramFiles%\PostgreSQL\<version>\data`, which has no named clusters — the
//! configured `cluster` is then only a label), with explicit overrides
//! (`data_dir`, `version`) for anything non-standard. No Tamanu coupling.

use std::path::{Path, PathBuf};

use miette::{Context as _, IntoDiagnostic as _, Result, bail};

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

/// A restore's placement, decided from the restored tree and the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePlan {
	/// The directory within the staging tree to move into place.
	pub source: PathBuf,
	/// Where it's moved to, keeping any existing dir as `<dest>.old`.
	pub dest: PathBuf,
	/// The cluster's major version, read from the restored `PG_VERSION`.
	pub data_major: String,
	/// Whether the snapshot carries the whole server install (`bin`/`lib` beside
	/// `data`, as the Windows backup now captures) rather than just the data dir.
	/// A whole-install restore brings its own matching binaries, so it needs no
	/// installed-version check.
	pub whole_install: bool,
}

/// Decide how to lay a restored tree (`staging`) down for `target`.
///
/// A whole-install snapshot (the data dir nested under a tree with a sibling
/// `bin`) replaces the whole server install directory, bringing its binaries. A
/// data-only snapshot (the data dir at the tree root) replaces just the cluster
/// data directory.
pub fn plan_restore(staging: &Path, target: &ResolvedCluster) -> Result<RestorePlan> {
	let data = locate_pgdata(staging)?;
	let data_major = read_pg_version(&data)?;

	// Whole-install only when the data dir is nested inside the restored tree and
	// its parent (also inside the tree) holds a `bin` directory — never when the
	// tree root *is* the data dir, where `data.parent()` would be a real host dir.
	let install_root = if data != staging {
		data.parent()
			.filter(|root| root.starts_with(staging) && root.join("bin").is_dir())
	} else {
		None
	};
	if let Some(install_root) = install_root {
		let dest = target.data_dir.parent().ok_or_else(|| {
			miette::miette!(
				"target data dir {} has no parent install directory to restore into",
				target.data_dir.display()
			)
		})?;
		return Ok(RestorePlan {
			source: install_root.to_path_buf(),
			dest: dest.to_path_buf(),
			data_major,
			whole_install: true,
		});
	}

	Ok(RestorePlan {
		source: data,
		dest: target.data_dir.clone(),
		data_major,
		whole_install: false,
	})
}

/// The cluster's major version from its `PG_VERSION` file (e.g. `18`, or `9.6`).
fn read_pg_version(data_dir: &Path) -> Result<String> {
	let raw = std::fs::read_to_string(data_dir.join("PG_VERSION"))
		.into_diagnostic()
		.wrap_err_with(|| format!("reading PG_VERSION in {}", data_dir.display()))?;
	Ok(raw.trim().to_owned())
}

/// The base directory server *binaries* live under, per major version:
/// `/usr/lib/postgresql` on Debian/Ubuntu, `%ProgramFiles%\PostgreSQL` (the same
/// tree as the data) on Windows.
fn server_base() -> PathBuf {
	#[cfg(windows)]
	{
		postgres_base()
	}
	#[cfg(not(windows))]
	{
		PathBuf::from("/usr/lib/postgresql")
	}
}

/// The installed server major versions (each a `<version>` dir with a `bin`
/// subdirectory under [`server_base`]), sorted.
pub fn installed_server_versions() -> Vec<String> {
	versions_under(&server_base())
}

/// The `<version>` dirs (with a `bin` subdirectory) directly under `base`, sorted.
fn versions_under(base: &Path) -> Vec<String> {
	let mut out: Vec<String> = std::fs::read_dir(base)
		.into_iter()
		.flatten()
		.flatten()
		.filter(|entry| entry.path().join("bin").is_dir())
		.filter_map(|entry| entry.file_name().into_string().ok())
		.collect();
	out.sort();
	out
}

/// Error unless server binaries for major `wanted` are installed. A data-only
/// backup doesn't carry binaries, and a physical restore only runs under its own
/// major version (minor differences are fine), so the matching server must be
/// present before the restore.
pub fn ensure_server_version_available(wanted: &str) -> Result<()> {
	ensure_version_present(wanted, &installed_server_versions())
}

fn ensure_version_present(wanted: &str, installed: &[String]) -> Result<()> {
	if installed.iter().any(|version| version == wanted) {
		return Ok(());
	}
	let found = if installed.is_empty() {
		"none found".to_owned()
	} else {
		format!("found {}", installed.join(", "))
	};
	bail!(
		"PostgreSQL {wanted} server binaries are not installed ({found}); a data-only \
		 backup restores only under its own major version. Install PostgreSQL {wanted} and retry."
	)
}

/// The directory a restore stages into: a sibling of what it will replace, on the
/// same filesystem so the swap is a rename. On Windows that's the `PostgreSQL`
/// base (so the whole `PostgreSQL\<version>` install can be replaced); elsewhere
/// the data dir's parent.
pub fn restore_staging_parent(config: &PostgresqlConfig) -> Option<PathBuf> {
	let target = resolve_target(config).ok()?;
	#[cfg(windows)]
	{
		target
			.data_dir
			.parent()
			.and_then(Path::parent)
			.map(Path::to_path_buf)
	}
	#[cfg(not(windows))]
	{
		target.data_dir.parent().map(Path::to_path_buf)
	}
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
			connection_url: None,
			port: None,
			socket: None,
			strategy: None,
			service_name: None,
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

	fn target(data_dir: PathBuf) -> ResolvedCluster {
		ResolvedCluster {
			version: "18".into(),
			cluster: "main".into(),
			data_dir,
		}
	}

	#[test]
	fn plan_restore_data_only_replaces_the_data_dir() {
		let tmp = tempfile::tempdir().unwrap();
		// A data-only snapshot: PG_VERSION at the staging root.
		let staging = tmp.path().join("staging");
		std::fs::create_dir_all(&staging).unwrap();
		std::fs::write(staging.join("PG_VERSION"), "18\n").unwrap();

		let data_dir = tmp.path().join("18").join("data");
		let plan = plan_restore(&staging, &target(data_dir.clone())).unwrap();
		assert!(!plan.whole_install);
		assert_eq!(plan.source, staging);
		assert_eq!(plan.dest, data_dir);
		assert_eq!(plan.data_major, "18");
	}

	#[test]
	fn plan_restore_whole_install_replaces_the_install_root() {
		let tmp = tempfile::tempdir().unwrap();
		// A whole-install snapshot: bin/ beside data/PG_VERSION at the staging root.
		let staging = tmp.path().join("staging");
		std::fs::create_dir_all(staging.join("bin")).unwrap();
		let data = staging.join("data");
		std::fs::create_dir_all(&data).unwrap();
		std::fs::write(data.join("PG_VERSION"), "18").unwrap();

		let target_data = tmp.path().join("PostgreSQL").join("18").join("data");
		let plan = plan_restore(&staging, &target(target_data.clone())).unwrap();
		assert!(plan.whole_install);
		assert_eq!(plan.source, staging);
		assert_eq!(plan.dest, target_data.parent().unwrap());
		assert_eq!(plan.data_major, "18");
	}

	#[test]
	fn versions_under_lists_dirs_with_a_bin() {
		let tmp = tempfile::tempdir().unwrap();
		std::fs::create_dir_all(tmp.path().join("16").join("bin")).unwrap();
		std::fs::create_dir_all(tmp.path().join("18").join("bin")).unwrap();
		// No bin: not a server install.
		std::fs::create_dir_all(tmp.path().join("junk")).unwrap();
		assert_eq!(versions_under(tmp.path()), vec!["16".to_owned(), "18".to_owned()]);
	}

	#[test]
	fn ensure_version_present_checks_membership() {
		let installed = vec!["16".to_owned(), "18".to_owned()];
		assert!(ensure_version_present("18", &installed).is_ok());
		let err = ensure_version_present("17", &installed).unwrap_err();
		let msg = format!("{err}");
		assert!(msg.contains("PostgreSQL 17"));
		assert!(msg.contains("found 16, 18"));
	}
}
