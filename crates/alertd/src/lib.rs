use std::{fmt, sync::Arc, time::Duration};

pub use bestool_canopy as canopy;
pub use bestool_canopy::Redacted;

pub mod backup;
mod child_confinement;
pub mod commands;
mod context;
mod daemon;
pub mod doctor;
pub mod http_server;
mod metrics;
pub mod tasks;

#[cfg(windows)]
pub mod windows_service;

pub use backup::{BackupRegistry, BackupRunner, BackupTask, RunningBackup};
pub use context::InternalContext;
pub use daemon::{RestartTrigger, run, run_with_shutdown};
pub use tasks::{BackgroundTask, TaskContext, TaskEndpoint, TaskEndpointResponse};

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
/// Configuration for the alertd daemon
#[derive(Clone)]
pub struct DaemonConfig {
	/// Database connection pool, opened by the caller.
	///
	/// Centralising pool creation at the caller lets `bestool alertd`
	/// reuse the pool for one-off setup queries (kind detection, device key
	/// lookup) instead of opening additional short-lived connections.
	///
	/// `None` on hosts with no Tamanu deployment (and therefore no database).
	pub pg_pool: Option<bestool_postgres::pool::PgPool>,

	/// Database connection URL, retained for redacted display.
	pub database_url: Option<String>,

	/// Tamanu device key PEM, used as the client identity for canopy.
	///
	/// Held only long enough to build the canopy `reqwest::Client` at startup,
	/// then dropped. Wrapped in `Redacted` so debug-logging the config can't
	/// leak the key.
	pub device_key_pem: Option<Redacted<String>>,

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

	/// Backup run registry, set when backups are compiled in. Surfaced via the
	/// daemon's status so an operator can see what's backing up right now.
	pub backups: Option<Arc<BackupRegistry>>,

	/// Version of the running `bestool` binary, shown in the systemd status line.
	///
	/// Distinct from this crate's own [`VERSION`]: `bestool` and `bestool-alertd`
	/// are versioned independently, so the caller threads in its own version.
	pub binary_version: String,
}

impl fmt::Debug for DaemonConfig {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("DaemonConfig")
			.field("database_url", &self.database_url)
			.field("device_key_pem", &self.device_key_pem)
			.field("binary_version", &self.binary_version)
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
		pg_pool: Option<bestool_postgres::pool::PgPool>,
		database_url: Option<String>,
	) -> Self {
		Self {
			pg_pool,
			database_url,
			device_key_pem: None,
			no_server: false,
			server_addrs: Vec::new(),
			watchdog_timeout: Some(Duration::from_secs(10 * 60)),
			background_tasks: Vec::new(),
			backups: None,
			// Fallback only; the binary sets its own version via
			// `with_binary_version`. This crate's version differs from bestool's.
			binary_version: VERSION.to_string(),
		}
	}

	/// Set the running binary's (bestool's) version for the status line.
	pub fn with_binary_version(mut self, version: String) -> Self {
		self.binary_version = version;
		self
	}

	pub fn with_task(mut self, task: Arc<dyn BackgroundTask>) -> Self {
		self.background_tasks.push(task);
		self
	}

	/// Attach the backup registry, so the daemon's status can list in-flight runs.
	pub fn with_backups(mut self, registry: Arc<BackupRegistry>) -> Self {
		self.backups = Some(registry);
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
