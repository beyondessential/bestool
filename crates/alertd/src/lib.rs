#![deny(rust_2018_idioms)]

mod alert;
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

use tracing::info;

/// Send a reload signal to a running alertd daemon
///
/// Tries to connect to the daemon's HTTP API at each of the provided addresses in order
/// until one succeeds. This is an alternative to SIGHUP that works on all platforms
/// including Windows.
pub async fn send_reload(addrs: &[std::net::SocketAddr]) -> miette::Result<()> {
	let client = reqwest::Client::new();

	let mut last_error = None;

	for addr in addrs {
		let url = format!("http://{}", addr);
		info!("checking if daemon is running at {}", url);

		// First, check if daemon is running by fetching status
		let status_response = match client.get(format!("{}/status", url)).send().await {
			Ok(resp) => resp,
			Err(e) => {
				info!("failed to connect to {}: {}", url, e);
				last_error = Some(e);
				continue;
			}
		};

		if !status_response.status().is_success() {
			info!(
				"daemon at {} returned status: {}",
				url,
				status_response.status()
			);
			continue;
		}

		let status: serde_json::Value = match status_response.json().await {
			Ok(s) => s,
			Err(e) => {
				info!("failed to parse status response from {}: {}", url, e);
				continue;
			}
		};

		// Verify it's the correct daemon
		if status.get("name").and_then(|n| n.as_str()) != Some("bestool-alertd") {
			info!(
				"unexpected daemon running at {}: {:?}",
				url,
				status.get("name")
			);
			continue;
		}

		info!(
			"found bestool-alertd daemon at {} (pid: {})",
			url,
			status.get("pid").unwrap_or(&serde_json::Value::Null)
		);

		// Send reload request
		info!("sending reload request to {}", url);
		let reload_response = match client.post(format!("{}/reload", url)).send().await {
			Ok(resp) => resp,
			Err(e) => {
				return Err(miette::miette!("reload request to {} failed: {}", url, e));
			}
		};

		if !reload_response.status().is_success() {
			return Err(miette::miette!(
				"reload request to {} failed (status: {})",
				url,
				reload_response.status()
			));
		}

		info!("reload request sent successfully to {}", url);
		return Ok(());
	}

	// If we get here, we couldn't connect to any address
	if let Some(err) = last_error {
		Err(miette::miette!(
			"failed to connect to daemon at any of {} address(es): {}",
			addrs.len(),
			err
		))
	} else {
		Err(miette::miette!(
			"no daemon found at any of {} address(es)",
			addrs.len()
		))
	}
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
