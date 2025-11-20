#![deny(rust_2018_idioms)]

mod alert;
mod daemon;
mod events;
mod glob_resolver;
pub mod http_server;
mod loader;
mod metrics;
mod pg_interval;
mod scheduler;
mod targets;
mod templates;

#[cfg(windows)]
pub mod windows_service;

pub use alert::{AlertDefinition, TicketSource};
pub use daemon::{run, run_with_shutdown};
pub use events::EventType;
pub use targets::{AlertTargets, ExternalTarget, SendTarget};

use miette::IntoDiagnostic;
use tracing::info;

/// Send a reload signal to a running alertd daemon
///
/// Connects to the daemon's HTTP API at http://127.0.0.1:8271 and triggers a reload.
/// This is an alternative to SIGHUP that works on all platforms including Windows.
pub async fn send_reload() -> miette::Result<()> {
	let client = reqwest::Client::new();

	// First, check if daemon is running by fetching status
	info!("checking if daemon is running at http://127.0.0.1:8271");
	let status_response = client
		.get("http://127.0.0.1:8271/status")
		.send()
		.await
		.into_diagnostic()?;

	if !status_response.status().is_success() {
		return Err(miette::miette!(
			"daemon not responding on http://127.0.0.1:8271 (status: {})",
			status_response.status()
		));
	}

	let status: serde_json::Value = status_response.json().await.into_diagnostic()?;

	// Verify it's the correct daemon
	if status.get("name").and_then(|n| n.as_str()) != Some("bestool-alertd") {
		return Err(miette::miette!(
			"unexpected daemon running on http://127.0.0.1:8271: {:?}",
			status.get("name")
		));
	}

	info!(
		"found bestool-alertd daemon (pid: {})",
		status.get("pid").unwrap_or(&serde_json::Value::Null)
	);

	// Send reload request
	info!("sending reload request");
	let reload_response = client
		.post("http://127.0.0.1:8271/reload")
		.send()
		.await
		.into_diagnostic()?;

	if !reload_response.status().is_success() {
		return Err(miette::miette!(
			"reload request failed (status: {})",
			reload_response.status()
		));
	}

	info!("reload request sent successfully");
	Ok(())
}

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
}

impl DaemonConfig {
	pub fn new(alert_globs: Vec<String>, database_url: String) -> Self {
		Self {
			alert_globs,
			database_url,
			email: None,
			dry_run: false,
			no_server: false,
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
}
