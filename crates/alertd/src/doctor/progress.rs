use tokio::sync::mpsc::UnboundedSender;

use super::check::CheckOutcome;

/// Progress events emitted while a doctor sweep runs.
#[derive(Debug, Clone)]
pub enum DoctorEvent {
	/// A check has produced a result, for the subject it was filed against.
	Completed(CheckOutcome),
}

pub type ProgressSender = UnboundedSender<DoctorEvent>;
