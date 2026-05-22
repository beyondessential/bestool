use std::collections::HashMap;

use reqwest::Url;

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
	pub fhir: Fhir,
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
		self.fhir.worker.enabled
	}
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
