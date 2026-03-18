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
