use tokio::sync::mpsc::UnboundedSender;

use super::check::Check;

/// Progress events emitted while a doctor sweep runs.
#[derive(Debug, Clone)]
pub enum DoctorEvent {
	/// A check has produced a result.
	Completed(Check),
}

pub type ProgressSender = UnboundedSender<DoctorEvent>;
