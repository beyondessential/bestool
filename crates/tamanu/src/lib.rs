use std::{
	fmt::Debug,
	path::{Path, PathBuf},
};

use miette::{IntoDiagnostic, Result, miette};
use node_semver::Version;
use tracing::{debug, instrument};

pub mod config;
pub mod connection_url;
pub mod pm2;
pub mod roots;
pub mod server_info;
pub mod services;
pub mod versions;

pub mod systemd;

#[cfg(feature = "doctor")]
pub mod doctor;

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

#[instrument(level = "debug")]
pub fn find_tamanu(root: Option<&Path>) -> Result<(Version, PathBuf)> {
	#[inline]
	fn inner(root: Option<&Path>) -> Result<(Version, PathBuf)> {
		if let Some(root) = root {
			let version = roots::version_of_root(root)?
				.ok_or_else(|| miette!("no tamanu found in --root={root:?}"))?;
			Ok((version, root.canonicalize().into_diagnostic()?))
		} else {
			roots::find_versions()?
				.into_iter()
				.next()
				.ok_or_else(|| miette!("no tamanu discovered, use --root"))
		}
	}

	inner(root).inspect(|(version, root)| debug!(?root, ?version, "found Tamanu root"))
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
