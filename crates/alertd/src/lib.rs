pub use bestool_canopy as canopy;
pub use bestool_canopy::Redacted;

pub mod check;
pub mod checks;
pub mod heal;
pub mod progress;
pub mod server_info;
pub mod stat;
pub mod subject;
pub mod sweep;

pub use stat::{MetricsSnapshot, Stat, StatKind, StatusCounts};
pub use subject::{ApplicationKind, Subject, TamanuScope};
pub use sweep::{
	SweepResult, SweepTamanu, SweepTargets, discover_sweep_targets, overall_from_payload,
	perform_sweep, resolve_sweep_targets,
};

/// The version of the alertd library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What alertd identifies as on outbound requests.
///
/// Carries this crate's version rather than the calling binary's, so the
/// identity tracks the checks rather than whatever ships them. The daemon
/// builds its clients with it; canopy sets its own User-Agent, so this applies
/// to alertd's other requests.
pub const USER_AGENT: &str = concat!("bestool-alertd/", env!("CARGO_PKG_VERSION"));

#[cfg(test)]
mod tests {
	use super::{USER_AGENT, VERSION};

	/// The daemon builds its HTTP clients from this, and it lives here rather
	/// than in the binary so it carries the checks crate's version. Rebuilding
	/// it from the binary's own `CARGO_PKG_VERSION` would silently change what
	/// alertd identifies as.
	#[test]
	fn user_agent_carries_this_crates_version() {
		assert_eq!(USER_AGENT, format!("bestool-alertd/{VERSION}"));
	}
}
