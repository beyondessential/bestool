use std::{
	fmt::Debug,
	path::{Path, PathBuf},
};

use miette::{IntoDiagnostic, Result, miette};
use node_semver::Version;
use tracing::{debug, instrument, warn};

pub mod caddy;
pub mod config;
pub mod connection_url;
pub mod pm2;
pub mod roots;
pub mod server_info;
pub mod services;
pub mod versions;

pub mod systemd;

/// What kind of server to interact with.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApiServerKind {
	Central,
	Facility,
}

impl ApiServerKind {
	pub fn package_name(&self) -> &'static str {
		match self {
			Self::Central => "central-server",
			Self::Facility => "facility-server",
		}
	}

	pub fn from_str_ci(s: &str) -> Option<Self> {
		match s {
			"central" | "central-server" => Some(Self::Central),
			"facility" | "facility-server" => Some(Self::Facility),
			_ => None,
		}
	}
}

/// Whether this root is a `/etc/tamanu/<version>` config dir, whose name says
/// nothing about the active version (one can be pre-staged ahead of an
/// upgrade or left behind by a rollback).
fn is_config_dir(root: &Path) -> bool {
	cfg!(target_os = "linux") && root.starts_with("/etc/tamanu")
}

const ENV_FILE: &str = "/etc/tamanu/env";

/// Discover the Tamanu install on this host.
///
/// `Ok(None)` means the host carries no trace of Tamanu: on Linux, no
/// `/etc/tamanu/env` (which every Tamanu deployment has) and no
/// package.json-versioned root (dev checkout, container, bare-metal install);
/// elsewhere, no known root at all. Callers like the doctor treat that as
/// "not a Tamanu host" and skip rather than fail.
///
/// `Err` means there *is* a Tamanu presence but it can't be resolved (invalid
/// `--root`, or an env file whose deployment doesn't match any config dir).
#[instrument(level = "debug")]
pub async fn try_find_tamanu(root: Option<&Path>) -> Result<Option<(Version, PathBuf)>> {
	if let Some(root) = root {
		let version = roots::version_of_root(root)?
			.ok_or_else(|| miette!("no tamanu found in --root={root:?}"))?;
		let root = root.canonicalize().into_diagnostic()?;
		debug!(?root, ?version, "found Tamanu root");
		return Ok(Some((version, root)));
	}

	let mut candidates = roots::find_versions()?;
	gate_config_dirs(&mut candidates, Path::new(ENV_FILE).exists());

	// Config dirs are only selectable when a live signal corroborates them.
	if candidates.iter().any(|(_, root)| is_config_dir(root)) {
		if let Some((version, root)) = select_active(&candidates).await {
			debug!(?root, ?version, "found active Tamanu root");
			return Ok(Some((version, root)));
		}

		// Other roots carry their version intrinsically (package.json), so
		// they remain selectable without a signal.
		return candidates
			.into_iter()
			.find(|(_, root)| !is_config_dir(root))
			.inspect(|(version, root)| debug!(?root, ?version, "found Tamanu root"))
			.ok_or_else(|| {
				miette!(
					"cannot determine the active Tamanu version: no running container, \
					database record, or env file matches a config dir under /etc/tamanu; \
					use --root"
				)
			})
			.map(Some);
	}

	Ok(candidates
		.into_iter()
		.next()
		.inspect(|(version, root)| debug!(?root, ?version, "found Tamanu root")))
}

/// [`try_find_tamanu`], erroring when the host has no Tamanu at all. For
/// commands that can't do anything without an install.
pub async fn find_tamanu(root: Option<&Path>) -> Result<(Version, PathBuf)> {
	try_find_tamanu(root).await?.ok_or_else(|| {
		if cfg!(target_os = "linux") && !Path::new(ENV_FILE).exists() {
			miette!("not a Tamanu host: {ENV_FILE} not found (use --root to override)")
		} else {
			miette!("no tamanu discovered, use --root")
		}
	})
}

/// A Tamanu host always carries the env file; without it, config dirs under
/// /etc/tamanu don't indicate a deployment at all and are dropped.
fn gate_config_dirs(candidates: &mut Vec<(Version, PathBuf)>, env_present: bool) {
	if env_present {
		return;
	}
	candidates.retain(|(_, root)| {
		let keep = !is_config_dir(root);
		if !keep {
			debug!(
				?root,
				"ignoring config dir: no {ENV_FILE}, not a Tamanu host"
			);
		}
		keep
	});
}

/// Match the deployment's active version to a discovered root.
///
/// Signals, most authoritative first:
/// 1. the running API container's image tag (what is running),
/// 2. the database's `currentVersion` fact (what last ran),
/// 3. `/etc/tamanu/env`'s `TAMANU_VERSION` (what will start).
async fn select_active(candidates: &[(Version, PathBuf)]) -> Option<(Version, PathBuf)> {
	let matching = |version: &Version| candidates.iter().find(|(v, _)| v == version).cloned();

	if let Some(running) = versions::running_version().await {
		match matching(&running) {
			Some(found) => return Some(found),
			None => warn!(%running, "running version has no discovered root"),
		}
	}

	// Reading the DB requires a config, which we don't have a root for yet;
	// DB credentials don't vary between versioned config dirs, so any
	// candidate's config will do.
	if let Some((_, provisional)) = candidates.first()
		&& let Ok(config) = config::load_config(provisional, None)
		&& let Some(last_ran) = versions::db_current_version(&config.database_url()).await
	{
		match matching(&last_ran) {
			Some(found) => return Some(found),
			None => warn!(%last_ran, "database version has no discovered root"),
		}
	}

	if let Some(configured) = versions::env_file_version(Path::new(ENV_FILE)) {
		match matching(&configured) {
			Some(found) => return Some(found),
			None => warn!(%configured, "configured version has no discovered root"),
		}
	}

	None
}

/// Decide whether a Tamanu install is a facility or a central server.
///
/// Two complementary signals, in order of trust:
///
/// 1. **Tamanu's local DB**, when available. The `local_system_facts` table
///    carries `facilityIds` and `syncHost` rows that the application itself
///    only populates on facility servers. This is the most authoritative
///    signal we have, since it reflects what the running Tamanu actually
///    thinks it is — independent of config-file vintage or layout.
/// 2. **The loaded config** (`TamanuConfig::is_facility`), which is checked
///    when the DB signal isn't conclusive (no DB client, or no matching
///    rows). Looks at three independent fields: `serverFacilityIds` (plural,
///    new), `serverFacilityId` (singular, legacy — still in real-world
///    facility use), and `sync.host` (only set on facilities).
///
/// On disagreement between DB and config, the DB wins and we log a debug
/// breadcrumb. Filesystem-based detection (looking at `packages/<name>`) was
/// previously used here but is too fragile — container-based deployments
/// don't have a `packages/` directory at all, and stale package directories
/// on bare-metal installs misclassify the host.
#[instrument(level = "debug", skip(db_client))]
pub async fn detect_kind(
	config: &config::TamanuConfig,
	db_client: Option<&tokio_postgres::Client>,
) -> ApiServerKind {
	let config_says = if config.is_facility() {
		Some(ApiServerKind::Facility)
	} else {
		None
	};

	let db_says = match db_client {
		Some(c) => detect_kind_from_db(c).await,
		None => None,
	};

	match (db_says, config_says) {
		(Some(db), Some(cfg)) if db != cfg => {
			debug!(?db, ?cfg, "DB and config disagree on kind; trusting DB");
			db
		}
		(Some(k), _) | (_, Some(k)) => k,
		(None, None) => {
			debug!("no facility signal from DB or config; assuming central");
			ApiServerKind::Central
		}
	}
}

/// Look at `local_system_facts` for facility-only keys.
///
/// `facilityIds` and `syncHost` are written by Tamanu's facility codepaths;
/// their presence is a positive "this is a facility" signal. Absence isn't
/// conclusive (the rows might just not have landed yet on a brand-new install),
/// so we return `None` rather than `Some(Central)` when no facility key is
/// found — leaving the caller to fall back to config-based detection.
async fn detect_kind_from_db(client: &tokio_postgres::Client) -> Option<ApiServerKind> {
	match client
		.query_opt(
			"SELECT 1 FROM local_system_facts WHERE key IN ('facilityIds', 'syncHost') LIMIT 1",
			&[],
		)
		.await
	{
		Ok(Some(_)) => Some(ApiServerKind::Facility),
		Ok(None) => None,
		Err(err) => {
			debug!(%err, "could not query local_system_facts for kind detection");
			None
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	#[cfg(target_os = "linux")]
	fn gate_drops_config_dirs_without_env_file() {
		let v = |s: &str| Version::parse(s).unwrap();
		let mut candidates = vec![
			(v("2.55.4"), PathBuf::from("/etc/tamanu/v2.55.4")),
			(v("2.48.5"), PathBuf::from("/etc/tamanu/v2.48.5")),
			(v("2.40.0"), PathBuf::from("/home/dev/tamanu")),
		];

		let mut kept = candidates.clone();
		gate_config_dirs(&mut kept, true);
		assert_eq!(kept.len(), 3, "env file present keeps config dirs");

		gate_config_dirs(&mut candidates, false);
		assert_eq!(
			candidates,
			vec![(v("2.40.0"), PathBuf::from("/home/dev/tamanu"))],
			"no env file drops config dirs but keeps intrinsically-versioned roots"
		);
	}

	fn cfg_from(json: serde_json::Value) -> config::TamanuConfig {
		serde_json::from_value(json).expect("test config should parse")
	}

	#[tokio::test]
	async fn no_db_no_signal_defaults_to_central() {
		// Config without any facility marker and no DB → central. That's
		// the "fresh empty config" path; we used to return Facility from
		// `find_package` here, which is wrong on container deployments.
		let cfg = cfg_from(serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
		}));
		assert_eq!(detect_kind(&cfg, None).await, ApiServerKind::Central);
	}

	#[tokio::test]
	async fn config_facility_signal_without_db() {
		let cfg = cfg_from(serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
			"serverFacilityIds": ["f1"],
		}));
		assert_eq!(detect_kind(&cfg, None).await, ApiServerKind::Facility);
	}

	#[tokio::test]
	async fn legacy_singular_field_detected_without_db() {
		// The real-world case the user flagged: a facility using the
		// pre-multi-facility singular `serverFacilityId` field was being
		// reported as central. After this change, no DB call needed.
		let cfg = cfg_from(serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
			"serverFacilityId": "f1",
		}));
		assert_eq!(detect_kind(&cfg, None).await, ApiServerKind::Facility);
	}

	#[tokio::test]
	async fn sync_host_detected_without_db() {
		let cfg = cfg_from(serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
			"sync": { "host": "https://central.example.org" },
		}));
		assert_eq!(detect_kind(&cfg, None).await, ApiServerKind::Facility);
	}
}
