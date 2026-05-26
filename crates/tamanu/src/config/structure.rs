use std::collections::HashMap;

use url::Url;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TamanuConfig {
	pub canonical_host_name: Option<Url>,
	pub canonical_url: Option<Url>,
	/// Current (multi-facility) form. Newer installs only.
	pub server_facility_ids: Option<Vec<String>>,
	/// Legacy single-facility form. Still in use on older facility installs.
	pub server_facility_id: Option<String>,
	pub db: Database,
	pub mailgun: Option<Mailgun>,
	pub primary_time_zone: Option<String>,
	pub country_time_zone: Option<String>,
	#[serde(default)]
	pub integrations: Integrations,
	/// `sync` block. Only configured on facility servers — central servers have
	/// no sync target, so its presence is a reliable "is this a facility" signal.
	pub sync: Option<Sync>,
}

impl TamanuConfig {
	pub fn canonical_url(&self) -> Option<&Url> {
		self.canonical_host_name
			.as_ref()
			.or(self.canonical_url.as_ref())
	}

	/// Identify the server as a facility from any one of three independent
	/// config signals.
	///
	/// Central servers have *none* of these populated; any one being present is
	/// taken as facility. We check all three because real-world facility
	/// installs vary by Tamanu vintage:
	///
	///   * `serverFacilityIds` (plural) — current multi-facility form
	///   * `serverFacilityId` (singular) — legacy single-facility form, still
	///     in active use on older facility installs
	///   * `sync.host` — only configured on facilities (centrals have no
	///     upstream to sync to)
	pub fn is_facility(&self) -> bool {
		self.server_facility_ids
			.as_ref()
			.is_some_and(|ids| !ids.is_empty())
			|| self
				.server_facility_id
				.as_ref()
				.is_some_and(|id| !id.is_empty())
			|| self.sync.as_ref().is_some_and(|s| s.host.is_some())
	}

	/// Mirrors `getPrimaryTimeZone()` in Tamanu: prefers `primaryTimeZone`,
	/// falls back to `countryTimeZone`.
	pub fn primary_time_zone(&self) -> Option<&str> {
		self.primary_time_zone
			.as_deref()
			.or(self.country_time_zone.as_deref())
	}

	pub fn fhir_worker_enabled(&self) -> bool {
		self.integrations.fhir.worker.enabled
	}

	/// Build the `postgresql://` URL from `db` for the canonical
	/// username/password the local Tamanu install uses. Hosts default to
	/// `localhost`, port defers to the URL's libpq default when unset.
	///
	/// Subcommands that need a different role (e.g. psql's report-schema
	/// connections) build their own `ConnectionUrlBuilder` instead.
	pub fn database_url(&self) -> String {
		crate::connection_url::ConnectionUrlBuilder {
			username: self.db.username.clone(),
			password: Some(self.db.password.clone()),
			host: self
				.db
				.host
				.clone()
				.unwrap_or_else(|| "localhost".to_string()),
			port: self.db.port,
			database: self.db.name.clone(),
			ssl_mode: None,
		}
		.build()
	}
}

/// The `integrations` block of the Tamanu config.
///
/// Tamanu groups all third-party / optional subsystem toggles here. The only
/// field we currently care about is `fhir` (so we know whether the FHIR worker
/// services should be Up or Down), but the struct exists as its own type so
/// future integration probes have an obvious home.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Integrations {
	pub fhir: Fhir,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sync {
	/// URL of the central server this facility syncs against. Only set on
	/// facility servers.
	pub host: Option<Url>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Fhir {
	pub worker: FhirWorker,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FhirWorker {
	pub enabled: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Database {
	pub host: Option<String>,
	pub port: Option<u16>,
	pub name: String,
	pub username: String,
	pub password: String,
	pub report_schemas: Option<ReportSchemas>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReportSchemas {
	pub connections: HashMap<String, ReportConnection>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReportConnection {
	pub username: String,
	pub password: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailgun {
	pub domain: String,
	pub api_key: String,

	#[serde(rename = "from")]
	pub sender: String,
}

#[cfg(test)]
mod tests {
	use super::*;

	fn parse(json: serde_json::Value) -> TamanuConfig {
		serde_json::from_value(json).expect("test config should parse")
	}

	fn base() -> serde_json::Value {
		serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
		})
	}

	#[test]
	fn central_has_no_facility_signals() {
		assert!(!parse(base()).is_facility());
	}

	#[test]
	fn server_facility_ids_plural_marks_facility() {
		let mut json = base();
		json["serverFacilityIds"] = serde_json::json!(["facility-x"]);
		assert!(parse(json).is_facility());
	}

	#[test]
	fn empty_server_facility_ids_does_not_mark_facility() {
		let mut json = base();
		json["serverFacilityIds"] = serde_json::json!([]);
		assert!(!parse(json).is_facility());
	}

	#[test]
	fn legacy_singular_server_facility_id_marks_facility() {
		// Older facility installs use the singular form. This was the case
		// that misclassified a real facility as central in production.
		let mut json = base();
		json["serverFacilityId"] = serde_json::json!("facility-x");
		assert!(parse(json).is_facility());
	}

	#[test]
	fn empty_singular_server_facility_id_does_not_mark_facility() {
		let mut json = base();
		json["serverFacilityId"] = serde_json::json!("");
		assert!(!parse(json).is_facility());
	}

	#[test]
	fn sync_host_marks_facility() {
		// Central servers have no upstream to sync to; presence of `sync.host`
		// is a positive facility signal independent of the serverFacilityId(s)
		// fields.
		let mut json = base();
		json["sync"] = serde_json::json!({ "host": "https://central.example.org" });
		assert!(parse(json).is_facility());
	}

	#[test]
	fn empty_sync_block_does_not_mark_facility() {
		let mut json = base();
		json["sync"] = serde_json::json!({});
		assert!(!parse(json).is_facility());
	}

	#[test]
	fn fhir_worker_enabled_reads_integrations_path() {
		let mut json = base();
		json["integrations"] = serde_json::json!({ "fhir": { "worker": { "enabled": true } } });
		assert!(parse(json).fhir_worker_enabled());
	}

	#[test]
	fn fhir_worker_disabled_when_integrations_says_false() {
		let mut json = base();
		json["integrations"] = serde_json::json!({ "fhir": { "worker": { "enabled": false } } });
		assert!(!parse(json).fhir_worker_enabled());
	}

	#[test]
	fn fhir_worker_disabled_when_integrations_missing() {
		assert!(!parse(base()).fhir_worker_enabled());
	}

	#[test]
	fn legacy_top_level_fhir_field_is_ignored() {
		// The old (wrong) location. We don't honour it — the actual schema
		// puts this under `integrations.fhir.worker.enabled`, and pretending
		// the top-level key still works would hide real misconfigurations.
		let mut json = base();
		json["fhir"] = serde_json::json!({ "worker": { "enabled": true } });
		assert!(!parse(json).fhir_worker_enabled());
	}
}
