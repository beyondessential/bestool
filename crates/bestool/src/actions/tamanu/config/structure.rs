use std::collections::HashMap;

use reqwest::Url;

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TamanuConfig {
	pub canonical_host_name: Option<Url>,
	pub db: Database,
	pub mailgun: Option<Mailgun>,
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
