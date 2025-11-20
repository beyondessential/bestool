#![deny(rust_2018_idioms)]

mod alert;
mod daemon;
mod events;
mod glob_resolver;
mod http_server;
mod loader;
mod metrics;
mod pg_interval;
mod scheduler;
mod targets;
mod templates;

pub use alert::{AlertDefinition, TicketSource};
pub use daemon::run;
pub use events::EventType;
pub use targets::{AlertTargets, ExternalTarget, SendTarget};

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
}

impl DaemonConfig {
	pub fn new(alert_globs: Vec<String>, database_url: String) -> Self {
		Self {
			alert_globs,
			database_url,
			email: None,
			dry_run: false,
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
}
