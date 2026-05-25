use std::{collections::HashMap, sync::Arc, time::Duration};

use jiff::Timestamp;
use tokio::sync::mpsc;

use crate::{
	EmailConfig, alert::InternalContext, events::EventManager, scheduler::Scheduler,
	tasks::TaskEndpointHandler,
};

#[derive(Clone)]
pub struct ServerState {
	pub reload_tx: mpsc::Sender<()>,
	pub started_at: Timestamp,
	pub pid: u32,
	pub event_manager: Option<Arc<EventManager>>,
	pub internal_context: Arc<InternalContext>,
	pub email_config: Option<EmailConfig>,
	pub dry_run: bool,
	pub scheduler: Arc<Scheduler>,
	pub watchdog_timeout: Option<Duration>,
	/// Endpoint handlers exposed by registered background tasks. Keyed by
	/// `(task_name, endpoint_name)` so the `/tasks/:task/:endpoint` route can
	/// dispatch in O(1) without walking the task registry per request.
	pub task_endpoints: Arc<HashMap<(String, String), TaskEndpointHandler>>,
}
