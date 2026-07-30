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

/// The `/seedling` response: what a co-located tool needs to know about this
/// host's Seedling access.
#[derive(Serialize, Deserialize)]
pub struct SeedlingResponse {
	/// Whether the host carries a Seedling identity for bestool, which a
	/// privileged process can use to reach the daemon. Never the identity
	/// itself: a tool asks this to learn that elevating will get it one.
	pub host_identity: bool,
}

/// The `/health` response: the watchdog's view of the daemon's liveness.
#[derive(Serialize, Deserialize)]
pub struct HealthResponse {
	/// Whether the daemon is within its watchdog window (always true when the
	/// watchdog is disabled). The endpoint also signals this via the HTTP status.
	pub healthy: bool,
	/// Seconds since a background task last reported activity; `None` if none has
	/// yet (freshly started).
	pub last_activity_secs_ago: Option<i64>,
	/// Seconds since the daemon started.
	pub uptime_secs: i64,
	/// The configured watchdog timeout in seconds; `None` when the watchdog is
	/// disabled.
	pub watchdog_timeout_secs: Option<u64>,
}
