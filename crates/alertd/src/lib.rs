#![deny(rust_2018_idioms)]

use std::path::PathBuf;

mod alert;
mod daemon;
mod loader;
mod pg_interval;
mod scheduler;
mod targets;
mod templates;

pub use alert::{AlertDefinition, TicketSource};
pub use daemon::run;
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
	/// Directories containing alert definitions
	pub alert_dirs: Vec<PathBuf>,

	/// Database connection URL
	pub database_url: String,

	/// Email server configuration
	pub email: Option<EmailConfig>,

	/// Whether to perform a dry run (execute all alerts once and quit)
	pub dry_run: bool,
}

impl DaemonConfig {
	pub fn new(alert_dirs: Vec<PathBuf>, database_url: String) -> Self {
		Self {
			alert_dirs,
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
