use std::{fmt, sync::Arc, time::Duration};

pub use bestool_canopy as canopy;
pub use bestool_canopy::Redacted;

mod alert;
pub mod commands;
mod daemon;
pub mod doctor;
mod events;
mod glob_resolver;
pub mod http_server;
mod loader;
mod metrics;

pub mod scheduler;
pub mod state_file;
mod targets;
pub mod tasks;
pub mod templates;

#[cfg(windows)]
pub mod windows_service;

pub use alert::{
	AlertDefinition, AlwaysSend, InternalContext, TicketSource, WhenChanged, server_kind_matches,
};
pub use daemon::{run, run_with_shutdown, run_with_shutdown_and_reload};
pub use events::EventType;
pub use targets::{
	AlertTargets, ExternalTarget, ResolvedTarget, SendTarget, TargetConnection, TargetEmail,
};
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

/// Email server configuration
#[derive(Debug, Clone)]
pub struct EmailConfig {
	pub from: String,
	pub mailgun_api_key: String,
	pub mailgun_domain: String,
}

/// Configuration for the alertd daemon
#[derive(Clone)]
pub struct DaemonConfig {
	/// Glob patterns for directories/files containing alert definitions
	///
	/// Patterns are resolved to directories and files, and watched for changes.
	/// On occasion, patterns are re-evaluated to pick up newly created paths.
	pub alert_globs: Vec<String>,

	/// Database connection pool, opened by the caller.
	///
	/// Centralising pool creation at the caller lets `bestool tamanu alertd`
	/// reuse the pool for one-off setup queries (kind detection, device key
	/// lookup) instead of opening additional short-lived connections.
	pub pg_pool: bestool_postgres::pool::PgPool,

	/// Database connection URL, retained for redacted display and as a
	/// substitution variable in alert templates (e.g. the `DatabaseDown`
	/// event context).
	pub database_url: String,

	/// Email server configuration
	pub email: Option<EmailConfig>,

	/// Tamanu device key PEM, used as the client identity for canopy targets.
	///
	/// Held only long enough to build the canopy `reqwest::Client` at startup,
	/// then dropped. Wrapped in `Redacted` so debug-logging the config can't
	/// leak the key.
	pub device_key_pem: Option<Redacted<String>>,

	/// Tamanu version of the install this daemon is alerting for. Sent in the
	/// `X-Version` header on every canopy request — canopy rejects requests
	/// without one.
	pub tamanu_version: String,

	/// Whether to perform a dry run (execute all alerts once and quit)
	pub dry_run: bool,

	/// Whether to disable the HTTP server
	pub no_server: bool,

	/// HTTP server bind addresses
	pub server_addrs: Vec<std::net::SocketAddr>,

	/// Watchdog timeout duration
	///
	/// If no alert task reports activity within this duration, the daemon
	/// will exit with an error so it can be restarted by the service manager.
	/// Set to `None` to disable the watchdog.
	pub watchdog_timeout: Option<Duration>,

	/// Background tasks to run on a schedule alongside the alert scheduler.
	///
	/// Each task ticks at its own `interval()`. Errors are logged but do not
	/// kill the daemon. Activity from each tick counts towards the watchdog.
	pub background_tasks: Vec<Arc<dyn BackgroundTask>>,

	/// Opaque label identifying this daemon's deployment role, used to
	/// filter alert definitions by their `server-kind:` field. Alertd is
	/// agnostic about what the string means — it's whatever the configurer
	/// (e.g. `bestool tamanu alertd`) decides to pass through. `None` means
	/// "no filtering": every alert applies regardless of its declared
	/// `server-kind`.
	pub server_kind: Option<String>,
}

impl fmt::Debug for DaemonConfig {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("DaemonConfig")
			.field("alert_globs", &self.alert_globs)
			.field("database_url", &self.database_url)
			.field("email", &self.email)
			.field("device_key_pem", &self.device_key_pem)
			.field("tamanu_version", &self.tamanu_version)
			.field("dry_run", &self.dry_run)
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
			.field("server_kind", &self.server_kind)
			.finish()
	}
}

impl DaemonConfig {
	pub fn new(
		alert_globs: Vec<String>,
		pg_pool: bestool_postgres::pool::PgPool,
		database_url: String,
		tamanu_version: String,
	) -> Self {
		Self {
			alert_globs,
			pg_pool,
			database_url,
			email: None,
			device_key_pem: None,
			tamanu_version,
			dry_run: false,
			no_server: false,
			server_addrs: Vec::new(),
			watchdog_timeout: Some(Duration::from_secs(10 * 60)),
			background_tasks: Vec::new(),
			server_kind: None,
		}
	}

	pub fn with_server_kind(mut self, kind: impl Into<String>) -> Self {
		self.server_kind = Some(kind.into());
		self
	}

	pub fn with_task(mut self, task: Arc<dyn BackgroundTask>) -> Self {
		self.background_tasks.push(task);
		self
	}

	pub fn with_email(mut self, email: EmailConfig) -> Self {
		self.email = Some(email);
		self
	}

	pub fn with_device_key_pem(mut self, pem: String) -> Self {
		self.device_key_pem = Some(Redacted(pem));
		self
	}

	pub fn with_dry_run(mut self, dry_run: bool) -> Self {
		self.dry_run = dry_run;
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
