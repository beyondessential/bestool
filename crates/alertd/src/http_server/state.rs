use std::{collections::HashMap, sync::Arc, time::Duration};

use jiff::Timestamp;

use crate::{context::InternalContext, daemon::DaemonControl, tasks::TaskEndpointHandler};

#[derive(Clone)]
pub struct ServerState {
	pub started_at: Timestamp,
	pub pid: u32,
	/// Version of the running `bestool` binary (not this crate's version), so
	/// `/status` reports what self-update targets and operators recognise.
	pub binary_version: String,
	pub internal_context: Arc<InternalContext>,
	pub watchdog_timeout: Option<Duration>,
	/// Endpoint handlers exposed by registered background tasks. Keyed by
	/// `(task_name, endpoint_name)` so the `/tasks/:task/:endpoint` route can
	/// dispatch in O(1) without walking the task registry per request.
	pub task_endpoints: Arc<HashMap<(String, String), TaskEndpointHandler>>,
	/// Drives the daemon's `/reload` and `/restart` control endpoints.
	pub control: DaemonControl,
	/// Backup run registry, when backups are compiled in; lets `/status` list
	/// in-flight runs.
	pub backups: Option<Arc<crate::BackupRegistry>>,
	/// Handle to the doctor task's latest sweep, when a doctor task is
	/// registered; feeds per-check stats and the status census to `/metrics`.
	pub metrics: Option<crate::doctor::DoctorMetricsHandle>,
}
