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
pub use subject::{ApplicationKind, CheckScope, Subject};
pub use sweep::{
	SweepResult, SweepTamanu, discover_sweep_tamanu, overall_from_payload, perform_sweep,
	resolve_sweep_tamanu,
};

/// The version of the alertd library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Base builder for alertd's outbound HTTP clients. Call sites add their own
/// timeouts etc. Canopy sets its own User-Agent, so this one applies to alertd's
/// other requests.
pub fn http_builder() -> reqwest::ClientBuilder {
	reqwest::Client::builder().user_agent(concat!("bestool-alertd/", env!("CARGO_PKG_VERSION")))
}

/// A built [`reqwest::Client`] from [`http_builder`].
pub fn http_client() -> reqwest::Client {
	http_builder()
		.build()
		.expect("failed to build alertd HTTP client")
}
