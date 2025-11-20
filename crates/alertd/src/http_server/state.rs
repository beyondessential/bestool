use std::sync::Arc;

use jiff::Timestamp;
use tokio::sync::mpsc;

use crate::{EmailConfig, alert::InternalContext, events::EventManager, scheduler::Scheduler};

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
}
