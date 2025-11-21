use std::{
	collections::HashMap,
	convert::Infallible,
	env::current_dir,
	path::{Path, PathBuf},
	sync::Arc,
	time::Duration,
};

use clap::Parser;
use futures::{TryFutureExt, future::join_all};
use miette::{Context as _, IntoDiagnostic, Result};
use tokio::{task::JoinSet, time::timeout};
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

use super::{definition::AlertDefinition, targets::AlertTargets};
use crate::actions::{
	Context,
	tamanu::{TamanuArgs, config::load_config, find_tamanu},
};

/// Execute alert definitions against Tamanu
///
/// DEPRECATED. Use `bestool tamanu alertd` for all new deployments.
///
/// The alert and target definitions are documented online at:
/// <https://github.com/beyondessential/bestool/blob/main/crates/alertd/ALERTS.md>
/// and <https://github.com/beyondessential/bestool/blob/main/crates/alertd/TARGETS.md>.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct AlertsArgs {
	/// Folder containing alert definitions.
	///
	/// This folder will be read recursively for files with the `.yaml` or `.yml` extension.
	///
	/// Files that don't match the expected format will be skipped, as will files with
	/// `enabled: false` at the top level. Syntax errors will be reported for YAML files.
	///
	/// It's entirely valid to provide a folder that only contains a `_targets.yml` file.
	///
	/// Can be provided multiple times. Defaults to (depending on platform): `C:\Tamanu\alerts`,
	/// `C:\Tamanu\{current-version}\alerts`, `/opt/tamanu-toolbox/alerts`, `/etc/tamanu/alerts`,
	/// `/alerts`, and `./alerts`.
	#[arg(long)]
	pub dir: Vec<PathBuf>,

	/// How far back to look for alerts.
	///
	/// This is a duration string, e.g. `1d` for one day, `1h` for one hour, etc. It should match
	/// the task scheduling / cron interval for this command.
	#[arg(long, default_value = "15m")]
	pub interval: humantime::Duration,

	/// Timeout for each alert.
	///
	/// If an alert takes longer than this to query the database or run the shell script, it will be
	/// skipped. Defaults to 30 seconds.
	///
	/// This is a duration string, e.g. `1d` for one day, `1h` for one hour, etc.
	#[arg(long, default_value = "30s")]
	pub timeout: humantime::Duration,

	/// Don't actually send alerts, just print them to stdout.
	#[arg(long)]
	pub dry_run: bool,
}

pub struct InternalContext {
	pub pg_client: tokio_postgres::Client,
	pub http_client: reqwest::Client,
}

async fn default_dirs(root: &Path) -> Vec<PathBuf> {
	let mut dirs = vec![
		PathBuf::from(r"C:\Tamanu\alerts"),
		root.join("alerts"),
		PathBuf::from("/opt/tamanu-toolbox/alerts"),
		PathBuf::from("/etc/tamanu/alerts"),
		PathBuf::from("/alerts"),
	];
	if let Ok(cwd) = current_dir() {
		dirs.push(cwd.join("alerts"));
	}

	join_all(
		dirs.into_iter()
			.map(|dir| async { if dir.exists() { Some(dir) } else { None } }),
	)
	.await
	.into_iter()
	.flatten()
	.collect()
}

pub async fn run(ctx: Context<TamanuArgs, AlertsArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;
	let config = load_config(&root, None)?;
	debug!(?config, "parsed Tamanu config");

	let dirs = if ctx.args_sub.dir.is_empty() {
		default_dirs(&root).await
	} else {
		ctx.args_sub.dir
	};
	debug!(?dirs, "searching for alerts");

	let mut alerts = Vec::<AlertDefinition>::new();
	let mut external_targets = HashMap::new();
	for dir in dirs {
		let external_targets_path = dir.join("_targets.yml");
		if let Some(AlertTargets { targets }) = std::fs::read_to_string(&external_targets_path)
			.ok()
			.and_then(|content| {
				debug!(path=?external_targets_path, "parsing external targets");
				serde_yaml::from_str::<AlertTargets>(&content)
					.map_err(
						|err| warn!(path=?external_targets_path, "_targets.yml has errors! {err}"),
					)
					.ok()
			}) {
			for target in targets {
				external_targets
					.entry(target.id().into())
					.or_insert(Vec::new())
					.push(target);
			}
		}

		alerts.extend(
			WalkDir::new(dir)
				.into_iter()
				.filter_map(|e| e.ok())
				.filter(|e| e.file_type().is_file())
				.map(|entry| {
					let file = entry.path();

					if !file.extension().is_some_and(|e| e == "yaml" || e == "yml") {
						return Ok(None);
					}

					if file.file_stem().is_some_and(|n| n == "_targets") {
						return Ok(None);
					}

					debug!(?file, "parsing YAML file");
					let content = std::fs::read_to_string(file)
						.into_diagnostic()
						.wrap_err(format!("{file:?}"))?;
					let mut alert: AlertDefinition = serde_yaml::from_str(&content)
						.into_diagnostic()
						.wrap_err(format!("{file:?}"))?;

					alert.file = file.to_path_buf();
					alert.interval = ctx.args_sub.interval.into();
					debug!(?alert, "parsed alert file");
					Ok(if alert.enabled { Some(alert) } else { None })
				})
				.filter_map(|def: Result<Option<AlertDefinition>>| match def {
					Err(err) => {
						error!("{err:?}");
						None
					}
					Ok(def) => def,
				}),
		);
	}

	if alerts.is_empty() {
		info!("no alerts found, doing nothing");
		return Ok(());
	}

	if !external_targets.is_empty() {
		debug!(count=%external_targets.len(), "found some external targets");
	}

	for alert in &mut alerts {
		*alert = std::mem::take(alert).normalise(&external_targets);
	}
	debug!(count=%alerts.len(), "found some alerts");

	let mut pg_config = tokio_postgres::Config::default();
	pg_config.application_name(format!(
		"{}/{} (tamanu alerts)",
		env!("CARGO_PKG_NAME"),
		env!("CARGO_PKG_VERSION")
	));
	if let Some(host) = &config.db.host {
		pg_config.host(host);
	} else {
		pg_config.host("localhost");
	}
	pg_config.user(&config.db.username);
	pg_config.password(&config.db.password);
	pg_config.dbname(&config.db.name);
	info!(config=?pg_config, "connecting to Tamanu database");
	let (client, connection) = pg_config
		.connect(tokio_postgres::NoTls)
		.await
		.into_diagnostic()?;
	tokio::spawn(async move {
		if let Err(e) = connection.await {
			eprintln!("connection error: {}", e);
		}
	});

	let config = Arc::new(config);
	let internal_ctx = Arc::new(InternalContext {
		pg_client: client,
		http_client: reqwest::Client::new(),
	});

	let mut set = JoinSet::new();
	for alert in alerts {
		let internal_ctx = internal_ctx.clone();
		let dry_run = ctx.args_sub.dry_run;
		let timeout_d: Duration = ctx.args_sub.timeout.into();
		let name = alert.file.clone();
		let config = config.clone();
		set.spawn(
			timeout(timeout_d, async move {
				let error = format!("while executing alert: {}", alert.file.display());
				if let Err(err) = alert
					.execute(internal_ctx, &config, dry_run)
					.await
					.wrap_err(error)
				{
					eprintln!("{err:?}");
				}
			})
			.or_else(move |elapsed| async move {
				error!(alert=?name, "timeout: {elapsed:?}");
				Ok::<_, Infallible>(())
			}),
		);
	}

	while let Some(res) = set.join_next().await {
		if let Err(err) = res {
			error!("task: {err:?}");
		}
	}

	Ok(())
}
