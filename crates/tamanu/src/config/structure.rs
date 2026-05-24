use std::collections::HashMap;

use url::Url;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TamanuConfig {
	pub canonical_host_name: Option<Url>,
	pub canonical_url: Option<Url>,
	pub server_facility_ids: Option<Vec<String>>,
	pub db: Database,
	pub mailgun: Option<Mailgun>,
	pub primary_time_zone: Option<String>,
	pub country_time_zone: Option<String>,
	#[serde(default)]
	pub integrations: Integrations,
}

impl TamanuConfig {
	pub fn canonical_url(&self) -> Option<&Url> {
		self.canonical_host_name
			.as_ref()
			.or(self.canonical_url.as_ref())
	}

	pub fn is_facility(&self) -> bool {
		self.server_facility_ids
			.as_ref()
			.is_some_and(|ids| !ids.is_empty())
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
