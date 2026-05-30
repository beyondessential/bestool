use std::{collections::HashMap, sync::Arc, time::Duration};

use jiff::Timestamp;

use crate::{context::InternalContext, tasks::TaskEndpointHandler};

#[derive(Clone)]
pub struct ServerState {
	pub started_at: Timestamp,
	pub pid: u32,
	pub internal_context: Arc<InternalContext>,
	pub watchdog_timeout: Option<Duration>,
	/// Endpoint handlers exposed by registered background tasks. Keyed by
	/// `(task_name, endpoint_name)` so the `/tasks/:task/:endpoint` route can
	/// dispatch in O(1) without walking the task registry per request.
	pub task_endpoints: Arc<HashMap<(String, String), TaskEndpointHandler>>,
}
