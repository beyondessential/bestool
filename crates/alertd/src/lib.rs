#![deny(rust_2018_idioms)]

use std::path::PathBuf;

mod alert;
mod config;
mod daemon;
mod loader;
mod pg_interval;
mod scheduler;
mod targets;
mod templates;

pub use alert::{AlertDefinition, TicketSource};
pub use config::{Config, DatabaseConfig, EmailConfig};
pub use daemon::run;
pub use targets::{AlertTargets, ExternalTarget, SendTarget};

/// Configuration for the alertd daemon
#[derive(Debug, Clone)]
pub struct DaemonConfig {
	/// Directories containing alert definitions
	pub alert_dirs: Vec<PathBuf>,

	/// Database connection string
	pub database_url: String,

	/// Whether to perform a dry run (no actual sending)
	pub dry_run: bool,

	/// Whether to use colors in output
	pub use_colours: bool,
}

impl DaemonConfig {
	pub fn new(alert_dirs: Vec<PathBuf>, database_url: String) -> Self {
		Self {
			alert_dirs,
			database_url,
			dry_run: false,
			use_colours: true,
		}
	}

	pub fn with_dry_run(mut self, dry_run: bool) -> Self {
		self.dry_run = dry_run;
		self
	}

	pub fn with_colours(mut self, use_colours: bool) -> Self {
		self.use_colours = use_colours;
		self
	}
}
