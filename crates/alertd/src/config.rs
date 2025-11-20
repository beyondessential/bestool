use miette::{IntoDiagnostic, Result};
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Config {
	pub db: DatabaseConfig,
	pub email: Option<EmailConfig>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseConfig {
	pub host: Option<String>,
	pub username: String,
	pub password: String,
	pub name: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EmailConfig {
	pub from: String,
	pub mailgun_api_key: String,
	pub mailgun_domain: String,
}

impl Config {
	pub fn from_toml(content: &str) -> Result<Self> {
		toml::from_str(content).into_diagnostic()
	}

	pub fn from_json(content: &str) -> Result<Self> {
		serde_json::from_str(content).into_diagnostic()
	}
}
