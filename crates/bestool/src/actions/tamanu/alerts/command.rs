use std::{
	collections::HashMap,
	convert::Infallible,
	env::current_dir,
	path::{Path, PathBuf},
	sync::Arc,
	time::Duration,
};

use clap::Parser;
use futures::{future::join_all, TryFutureExt};
use miette::{Context as _, IntoDiagnostic, Result};
use tokio::{task::JoinSet, time::timeout};
use tracing::{debug, error, info, warn};
use walkdir::WalkDir;

use super::{definition::AlertDefinition, targets::AlertTargets};
use crate::actions::{
	tamanu::{config::load_config, find_tamanu, TamanuArgs},
	Context,
};

/// Execute alert definitions against Tamanu.
///
/// An alert definition is a YAML file that describes a single alert
/// source and targets to send triggered alerts to.
///
/// This tool reads both database and email credentials from Tamanu's own
/// configuration files, see the tamanu subcommand help (one level above) for
/// more on how that's determined.
///
/// # Example
///
/// ```yaml
/// sql: |
///  SELECT * FROM fhir.jobs
///  WHERE error IS NOT NULL
///  AND created_at > $1
///
/// send:
/// - target: email:
///   addresses: [alerts@tamanu.io]
///   subject: "FHIR job errors ({{ hostname }})"
///   template: |
///     Automated alert! There have been {{ rows | length }} FHIR jobs
///     with errors in the past {{ interval }}. Here are the first 5:
///     {% for row in rows | first(5) %}
///     - {{ row.topic }}: {{ row.error }}
///    {% endfor %}
/// ```
///
/// # Template variables
///
/// Templates are rendered with [Tera](https://keats.github.io/tera/docs/#templates),
/// and are expected to be either plain text or markdown.
///
/// - `rows`: the result of the SQL query, as a list of objects (if source = sql)
/// - `output`: the result of the shell command (if source = shell)
/// - `interval`: the duration string of the alert interval
/// - `hostname`: the hostname of the machine running this command
/// - `filename`: the name of the alert definition file
/// - `now`: the current date and time
///
/// Additionally you can `{% include "subject" %}` to include the rendering of
/// the subject template in the email template.
///
/// # Sources
///
/// Each alert must have one source that it executes to determine whether the
/// alert is triggered or not. Current sources: `sql`, `shell`.
///
/// ## SQL
///
/// This source executes a SQL query, which can have a binding for a datetime in
/// the past to limit results by; returning any rows indicates an alert trigger.
///
/// ```yaml
/// sql: |
///   SELECT 1 + 1
/// ```
///
/// ### Query binding parameters
///
/// The SQL query will be passed exactly the number of parameters it expects.
/// The parameters are always provided in this order:
///
/// - `$1`: the datetime of the start of the interval (timestamp with time zone)
/// - `$2`: the interval duration (interval)
///
/// ## Shell
///
/// This source executes a shell script. Returning a non-zero exit code
/// indicates an alert trigger. The stdout of the script will be the `output`
/// template variable.
///
/// ```yaml
/// shell: bash
/// run: |
///   echo foo
///   exit 1
/// ```
///
/// # Send targets
///
/// You can send triggered alerts to one or more different targets. Current send
/// targets are: `email`, `slack`, `zendesk`. Note that you can have multiple
/// targets of the same type.
///
/// ## Email
///
/// ```yaml
/// send:
///   - target: email
///     addresses:
///       - staff@job.com
///       - support@job.com
/// ```
///
/// The `template` field will be rendered and then converted to HTML (using markdown syntax).
///
/// ## Slack
///
/// ```yaml
/// send:
///   - target: slack
///     webhook: https://hooks.slack.com/services/...
///     template: |
///       _Alert!_ There are {{ rows | length }} rows with errors.
/// ```
///
/// You can customise the payload sent to Slack by specifying fields:
///
/// ```yaml
/// send:
///  - target: slack
///    webhook: https://hooks.slack.com/services/...
///    # ...
///    fields:
///    - name: alertname
///      field: filename # this will be replaced with the filename of the alert
///    - name: deployment
///      value: production # this will be the exact value 'production'
/// ```
///
/// The default set of fields is:
///
/// ```yaml
/// - name: hostname
///   field: hostname
/// - name: filename
///   field: filename
/// - name: subject
///   field: subject
/// - name: message
///   field: body
/// ```
///
/// Overriding the `fields` will replace the default set entirely (so you may
/// want to include all the ones you're not changing).
///
/// ## Zendesk (authenticated)
///
/// ```yaml
/// send:
///   - target: zendesk
///     endpoint: https://example.zendesk.com/api/v2/requests
///     credentials:
///       email: foo@example.com
///       password: pass
///     ticket_form_id: 500
///     custom_fields:
///       - id: 100
///         value: tamanu_
///       - id: 200
///         value: Test
/// ```
///
/// ## Zendesk (anonymous)
///
/// ```yaml
/// send:
///   - target: zendesk
///     endpoint: https://example.zendesk.com/api/v2/requests
///     requester: Name of requester
///     ticket_form_id: 500
///     custom_fields:
///       - id: 100
///         value: tamanu_
///       - id: 200
///         value: Test
/// ```
///
/// ## External targets
///
/// It can be tedious to specify and update the same addresses in many different
/// alert files, especially for more complex send targets. You can create a
/// `_targets.yml` file in any of the `--dir`s (if there are multiple such files
/// they will be merged).
///
/// ```yaml
/// targets:
///   - id: email-staff
///     target: email
///     addresses:
///       - staff@job.com
///   - id: zendesk-normal
///     target: zendesk
///     endpoint: https://...
///     credentials:
///       email: the@bear.com
///       password: ichooseyou
/// ```
///
/// The `subject` and `template` fields are omitted in the `_targets.yml`.
///
/// Then in the alerts file, specify `external` targets, with the relevant `id`s
/// and the `subject` and `template`:
///
/// ```yaml
/// send:
///   - target: external
///     id: email-staff
///     subject: [Alert] Something is wrong
///     template: |
///       # Whoops
/// ```
///
/// If you specify multiple external targets with the same `id`, the alert will be
/// multiplexed (i.e. sent to all targets with that `id`). This can be useful for
/// sending alerts to both email and slack, or for debugging by temporarily sending
/// alerts to an additional target.
///
/// ---
/// As this documentation is a bit hard to read in the terminal, you may want to
/// consult the online version:
/// <https://docs.rs/bestool/latest/bestool/__help/tamanu/alerts/struct.AlertsArgs.html>
#[cfg_attr(docsrs, doc("\n\n**Command**: `bestool tamanu alerts`"))]
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
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--interval DURATION`"))]
	#[arg(long, default_value = "30s")]
	pub timeout: humantime::Duration,

	/// Don't actually send alerts, just print them to stdout.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--dry-run`"))]
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

	join_all(dirs.into_iter().map(|dir| async {
		if dir.exists() {
			Some(dir)
		} else {
			None
		}
	}))
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
				serde_yml::from_str::<AlertTargets>(&content)
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

					if !file
						.extension()
						.map_or(false, |e| e == "yaml" || e == "yml")
					{
						return Ok(None);
					}

					if file.file_stem().map_or(false, |n| n == "_targets") {
						return Ok(None);
					}

					debug!(?file, "parsing YAML file");
					let content = std::fs::read_to_string(file)
						.into_diagnostic()
						.wrap_err(format!("{file:?}"))?;
					let mut alert: AlertDefinition = serde_yml::from_str(&content)
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
	pg_config.application_name(&format!(
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
		match res {
			Err(err) => {
				error!("task: {err:?}");
			}
			_ => (),
		}
	}

	Ok(())
}
