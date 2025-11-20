use std::path::PathBuf;

use clap::Parser;
use miette::Result;
use tracing::{debug, info};

use super::{TamanuArgs, config::load_config, connection_url::ConnectionUrlBuilder, find_tamanu};
use crate::actions::Context;

/// Run the alert daemon
///
/// This is a long-lived daemon that manages alert execution on scheduled intervals.
/// Unlike the `alerts` subcommand which is designed to run via cron, this daemon
/// manages its own timers and watches for configuration file changes.
///
/// The daemon will:
/// - Load alert definitions from specified directories
/// - Execute alerts on their configured intervals
/// - Watch for file system changes and reload automatically
/// - Reload on SIGHUP (Unix only)
/// - Gracefully shutdown on SIGINT/SIGTERM
///
/// Configuration for database and email is read from Tamanu's config files.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct AlertdArgs {
	/// Glob patterns for alert definitions.
	///
	/// Patterns can match directories (which will be read recursively) or individual files.
	/// Can be provided multiple times.
	/// Examples: /etc/tamanu/alerts, /opt/*/alerts, /etc/tamanu/alerts/**/*.yml
	#[arg(long)]
	pub dir: Vec<String>,

	/// Don't actually send alerts, just print them to stdout.
	#[arg(long)]
	pub dry_run: bool,
}

pub async fn run(ctx: Context<TamanuArgs, AlertdArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;
	let config = load_config(&root, None)?;
	debug!(?config, "parsed Tamanu config");

	let dirs = if ctx.args_sub.dir.is_empty() {
		default_dirs(&root).await
	} else {
		ctx.args_sub.dir.clone()
	};
	debug!(?dirs, "alert directories");

	if dirs.is_empty() {
		return Err(miette::miette!("no alert directories found or specified"));
	}

	info!("starting alertd daemon");

	let database_url = ConnectionUrlBuilder {
		username: config.db.username.clone(),
		password: Some(config.db.password.clone()),
		host: config
			.db
			.host
			.clone()
			.unwrap_or_else(|| "localhost".to_string()),
		port: config.db.port,
		database: config.db.name.clone(),
	}
	.build();

	let email = config
		.mailgun
		.as_ref()
		.map(|mg| bestool_alertd::EmailConfig {
			from: mg.sender.clone(),
			mailgun_api_key: mg.api_key.clone(),
			mailgun_domain: mg.domain.clone(),
		});

	let mut daemon_config =
		bestool_alertd::DaemonConfig::new(dirs, database_url).with_dry_run(ctx.args_sub.dry_run);

	if let Some(email) = email {
		daemon_config = daemon_config.with_email(email);
	}

	bestool_alertd::run(daemon_config).await
}

async fn default_dirs(root: &std::path::Path) -> Vec<String> {
	use futures::future::join_all;

	let mut dirs = vec![
		PathBuf::from(r"C:\Tamanu\alerts"),
		root.join("alerts"),
		PathBuf::from("/opt/tamanu-toolbox/alerts"),
		PathBuf::from("/etc/tamanu/alerts"),
		PathBuf::from("/alerts"),
	];
	if let Ok(cwd) = std::env::current_dir() {
		dirs.push(cwd.join("alerts"));
	}

	join_all(
		dirs.into_iter()
			.map(|dir| async { if dir.exists() { Some(dir) } else { None } }),
	)
	.await
	.into_iter()
	.flatten()
	.map(|p| p.display().to_string())
	.collect()
}
