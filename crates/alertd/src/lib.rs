use std::fmt;

mod alert;
pub mod commands;
mod daemon;
mod events;
mod glob_resolver;
pub mod http_server;
mod loader;
mod metrics;

pub mod scheduler;
mod targets;
pub mod templates;

#[cfg(windows)]
pub mod windows_service;

pub use alert::{AlertDefinition, InternalContext, TicketSource};
pub use daemon::{run, run_with_shutdown};
pub use events::EventType;
pub use targets::{AlertTargets, ExternalTarget, ResolvedTarget, SendTarget};

/// The version of the alertd library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Email server configuration
#[derive(Debug, Clone)]
pub struct EmailConfig {
	pub from: String,
	pub mailgun_api_key: String,
	pub mailgun_domain: String,
}

/// Configuration for the alertd daemon
#[derive(Debug, Clone)]
pub struct DaemonConfig {
	/// Glob patterns for directories/files containing alert definitions
	///
	/// Patterns are resolved to directories and files, and watched for changes.
	/// On occasion, patterns are re-evaluated to pick up newly created paths.
	pub alert_globs: Vec<String>,

	/// Database connection URL
	pub database_url: String,

	/// Email server configuration
	pub email: Option<EmailConfig>,

	/// Whether to perform a dry run (execute all alerts once and quit)
	pub dry_run: bool,

	/// Whether to disable the HTTP server
	pub no_server: bool,

	/// HTTP server bind addresses
	pub server_addrs: Vec<std::net::SocketAddr>,
}

impl DaemonConfig {
	pub fn new(alert_globs: Vec<String>, database_url: String) -> Self {
		Self {
			alert_globs,
			database_url,
			email: None,
			dry_run: false,
			no_server: false,
			server_addrs: Vec::new(),
		}
	}

	pub fn with_email(mut self, email: EmailConfig) -> Self {
		self.email = Some(email);
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
