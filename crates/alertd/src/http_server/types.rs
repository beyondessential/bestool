use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct StatusResponse {
	pub name: String,
	pub version: String,
	pub started_at: String,
	pub pid: u32,
}

#[derive(Deserialize)]
pub struct AlertRequest {
	pub message: String,
	#[serde(default)]
	pub subject: Option<String>,
	#[serde(flatten)]
	pub custom: serde_json::Value,
}

#[derive(Deserialize)]
pub struct PauseAlertRequest {
	pub alert: String,
	pub until: String,
}

#[derive(Serialize, Deserialize)]
pub struct ValidationResponse {
	pub valid: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub error: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub error_location: Option<ErrorLocation>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub info: Option<ValidationInfo>,
}

#[derive(Serialize, Deserialize)]
pub struct ErrorLocation {
	pub line: usize,
	pub column: usize,
	pub path: String,
}

#[derive(Serialize, Deserialize)]
pub struct ValidationInfo {
	pub enabled: bool,
	pub interval: String,
	pub source_type: String,
	pub targets: usize,
}

#[derive(Debug, Deserialize)]
pub struct AlertsQuery {
	#[serde(default)]
	pub detail: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AlertStateInfo {
	pub path: String,
	pub enabled: bool,
	pub interval: String,
	pub triggered_at: Option<String>,
	pub last_sent_at: Option<String>,
	pub paused_until: Option<String>,
	pub always_send: String,
}
