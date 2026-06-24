use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct StatusResponse {
	pub name: String,
	pub version: String,
	pub started_at: String,
	pub pid: u32,
	/// Backups running on this daemon right now (empty when none, or when
	/// backups aren't compiled in).
	#[serde(default)]
	pub backups_running: Vec<crate::RunningBackup>,
	/// Backup types configured on this host (empty when none, or when backups
	/// aren't compiled in).
	#[serde(default)]
	pub backups_configured: Vec<String>,
}
