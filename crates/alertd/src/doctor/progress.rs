use tokio::sync::mpsc::UnboundedSender;

use super::check::CheckOutcome;

/// Progress events emitted while a doctor sweep runs.
#[derive(Debug, Clone)]
pub enum DoctorEvent {
	/// Every check this sweep will run, identified by instance, in registry
	/// order. Sent once before any check completes.
	///
	/// The sweep is the only thing that knows which applications the host has —
	/// it resolves them from the deployment and the database it reaches — so it
	/// tells the display rather than the display detecting them a second time
	/// and risking a different answer.
	Planned(Vec<String>),
	/// A check has produced a result, for the subject it was filed against.
	Completed(CheckOutcome),
}

pub type ProgressSender = UnboundedSender<DoctorEvent>;
