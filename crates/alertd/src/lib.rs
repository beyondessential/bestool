use std::{fmt, sync::Arc, time::Duration};

pub use bestool_canopy as canopy;
pub use bestool_canopy::Redacted;

mod context;
mod daemon;
pub mod doctor;
pub mod http_server;
mod metrics;
pub mod tasks;

#[cfg(windows)]
pub mod windows_service;

pub use context::InternalContext;
pub use daemon::{run, run_with_shutdown};
pub use tasks::{BackgroundTask, TaskContext, TaskEndpoint, TaskEndpointResponse};

/// The version of the alertd library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Base builder for alertd's outbound HTTP clients.
///
/// Carries the browser-style `bestool/<version>` User-Agent (and whatever else
/// [`canopy::client_builder`] sets). Call sites add their own timeouts etc.
pub fn http_builder() -> reqwest::ClientBuilder {
	canopy::client_builder(VERSION)
}

/// A built [`reqwest::Client`] from [`http_builder`].
pub fn http_client() -> reqwest::Client {
	http_builder()
		.build()
		.expect("failed to build alertd HTTP client")
}
/// Configuration for the alertd daemon
#[derive(Clone)]
pub struct DaemonConfig {
	/// Database connection pool, opened by the caller.
	///
	/// Centralising pool creation at the caller lets `bestool tamanu alertd`
	/// reuse the pool for one-off setup queries (kind detection, device key
	/// lookup) instead of opening additional short-lived connections.
	pub pg_pool: bestool_postgres::pool::PgPool,

	/// Database connection URL, retained for redacted display.
	pub database_url: String,

	/// Tamanu device key PEM, used as the client identity for canopy.
	///
	/// Held only long enough to build the canopy `reqwest::Client` at startup,
	/// then dropped. Wrapped in `Redacted` so debug-logging the config can't
	/// leak the key.
	pub device_key_pem: Option<Redacted<String>>,

	/// Tamanu version of the install this daemon is monitoring. Sent in the
	/// `X-Version` header on every canopy request — canopy rejects requests
	/// without one.
	pub tamanu_version: String,

	/// Whether to disable the HTTP server
	pub no_server: bool,

	/// HTTP server bind addresses
	pub server_addrs: Vec<std::net::SocketAddr>,

	/// Watchdog timeout duration
	///
	/// If no background task reports activity within this duration, the daemon
	/// will exit with an error so it can be restarted by the service manager.
	/// Set to `None` to disable the watchdog.
	pub watchdog_timeout: Option<Duration>,

	/// Background tasks to run on a schedule.
	///
	/// Each task ticks at its own `interval()`. Errors are logged but do not
	/// kill the daemon. Activity from each tick counts towards the watchdog.
	pub background_tasks: Vec<Arc<dyn BackgroundTask>>,
}

impl fmt::Debug for DaemonConfig {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("DaemonConfig")
			.field("database_url", &self.database_url)
			.field("device_key_pem", &self.device_key_pem)
			.field("tamanu_version", &self.tamanu_version)
			.field("no_server", &self.no_server)
			.field("server_addrs", &self.server_addrs)
			.field("watchdog_timeout", &self.watchdog_timeout)
			.field(
				"background_tasks",
				&self
					.background_tasks
					.iter()
					.map(|t| t.name())
					.collect::<Vec<_>>(),
			)
			.finish()
	}
}

impl DaemonConfig {
	pub fn new(
		pg_pool: bestool_postgres::pool::PgPool,
		database_url: String,
		tamanu_version: String,
	) -> Self {
		Self {
			pg_pool,
			database_url,
			device_key_pem: None,
			tamanu_version,
			no_server: false,
			server_addrs: Vec::new(),
			watchdog_timeout: Some(Duration::from_secs(10 * 60)),
			background_tasks: Vec::new(),
		}
	}

	pub fn with_task(mut self, task: Arc<dyn BackgroundTask>) -> Self {
		self.background_tasks.push(task);
		self
	}

	pub fn with_device_key_pem(mut self, pem: String) -> Self {
		self.device_key_pem = Some(Redacted(pem));
		self
	}

	pub fn with_no_server(mut self, no_server: bool) -> Self {
		self.no_server = no_server;
		self
	}

	pub fn with_server_addrs(mut self, server_addrs: Vec<std::net::SocketAddr>) -> Self {
		self.server_addrs = server_addrs;
		self
	}

	pub fn with_watchdog_timeout(mut self, watchdog_timeout: Option<Duration>) -> Self {
		self.watchdog_timeout = watchdog_timeout;
		self
	}
}

/// Helper to format miette errors for logging without ANSI codes
pub(crate) struct LogError<'a>(pub &'a miette::Report);

impl fmt::Display for LogError<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		use miette::ReportHandler;

		let handler = miette::NarratableReportHandler::new();

		if let Err(e) = handler.debug(self.0.as_ref(), f) {
			write!(f, "{}: {}", self.0, e)
		} else {
			Ok(())
		}
	}
}
