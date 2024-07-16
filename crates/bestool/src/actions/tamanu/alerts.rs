use std::{path::PathBuf, time::Duration};

use clap::Parser;
use mailgun_rs::{EmailAddress, Mailgun, Message};
use miette::{Context as _, IntoDiagnostic, Result};
use sysinfo::System;
use tracing::{debug, info, instrument};
use walkdir::WalkDir;

use crate::{actions::Context, postgres_to_value::rows_to_value_map};

use super::{
	config::{merge_json, package_config},
	find_package, find_tamanu, TamanuArgs,
};

const DEFAULT_SUBJECT_TEMPLATE: &str = "[Tamanu Alert] {{ filename }} ({{ hostname }})";

/// Execute alert definitions against Tamanu.
///
/// An alert definition is a YAML file that describes a single alert
/// condition and recipients to notify. Conditions are expressed as a SQL query,
/// which must have a binding for a datetime in the past to limit results by;
/// returning any rows indicates a condition trigger. The result of the query is
/// sent to the recipients as an email, via a Tera template.
///
/// This tool reads both database and email credentials from Tamanu's own
/// configuration files, see the tamanu subcommand help (one level above) for
/// more on how that's determined.
///
/// # Example
///
/// ```yaml
/// enabled: true
/// recipients:
///   - alerts@tamanu.io
///
/// sql: |
///  SELECT * FROM fhir.jobs
///  WHERE error IS NOT NULL
///  AND created_at > $1
///
/// subject: "FHIR job errors ({{ hostname }})"
/// template: |
///   Automated alert! There have been {{ rows | length }} FHIR jobs
///   with errors in the past {{ interval }}. Here are the first 5:
///   {% for row in rows | first(5) %}
///   - {{ row.topic }}: {{ row.error }}
///  {% endfor %}
/// ```
///
/// # Template variables
///
/// - `rows`: the result of the SQL query, as a list of objects
/// - `interval`: the duration string of the alert interval
/// - `hostname`: the hostname of the machine running this command
/// - `filename`: the name of the alert definition file
/// - `now`: the current date and time
///
/// Additionally you can `{% include "subject" %}` to include the rendering of
/// the subject template in the email template.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct AlertsArgs {
	/// Folder containing alert definitions.
	///
	/// This folder will be read recursively for files with the `.yaml` or `.yml` extension.
	///
	/// Files that don't match the expected format will be skipped, as will files with
	/// `enabled: false`.
	#[arg(long)]
	pub dir: PathBuf,

	/// How far back to look for alerts.
	///
	/// This is a duration string, e.g. `1d` for one day, `1h` for one hour, etc. It should match
	/// the task scheduling / cron interval for this command.
	#[arg(long)]
	pub interval: humantime::Duration,

	/// Don't actually send emails, just print them to stdout.
	#[arg(long)]
	pub dry_run: bool,
}

#[derive(serde::Deserialize, Debug)]
struct TamanuConfig {
	db: TamanuDb,
	mailgun: TamanuMailgun,
}

#[derive(serde::Deserialize, Debug)]
struct TamanuDb {
	host: Option<String>,
	name: String,
	username: String,
	password: String,
}

#[derive(serde::Deserialize, Debug)]
struct TamanuMailgun {
	domain: String,
	#[serde(rename = "apiKey")]
	api_key: String,
	#[serde(rename = "from")]
	sender: String,
}

fn enabled() -> bool {
	true
}

#[derive(serde::Deserialize, Debug)]
struct AlertDefinition {
	#[serde(default, skip)]
	file: PathBuf,

	#[serde(default = "enabled")]
	enabled: bool,
	recipients: Vec<String>,
	sql: String,
	subject: Option<String>,
	template: String,
}

pub async fn run(ctx: Context<TamanuArgs, AlertsArgs>) -> Result<()> {
	let (_, root) = find_tamanu(&ctx.args_top)?;

	let kind = find_package(&root)?;
	info!(?root, ?kind, "using this Tamanu for config");

	let config_value = merge_json(
		package_config(&root, kind.package_name(), "default.json5")?,
		package_config(&root, kind.package_name(), "local.json5")?,
	);

	let config: TamanuConfig = serde_json::from_value(config_value)
		.into_diagnostic()
		.wrap_err("parsing of Tamanu config failed")?;
	debug!(?config, "parsed Tamanu config");

	let alerts: Vec<AlertDefinition> = WalkDir::new(ctx.args_sub.dir)
		.into_iter()
		.filter_map(|e| e.ok())
		.filter(|e| e.file_type().is_file())
		.filter_map(|entry| {
			let file = entry.path();
			if file
				.extension()
				.map_or(false, |e| e == "yaml" || e == "yml")
			{
				debug!(?file, "parsing YAML file");
				let content = std::fs::read_to_string(file).ok()?;
				let mut alert: AlertDefinition = serde_yml::from_str(&content).ok()?;
				debug!(?alert, "parsed alert file");
				if !alert.enabled {
					return None;
				}

				alert.file = file.to_path_buf();
				Some(alert)
			} else {
				None
			}
		})
		.collect();

	if alerts.is_empty() {
		info!("no alerts found, doing nothing");
		return Ok(());
	}
	debug!(count=%alerts.len(), "found some alerts");

	let now = chrono::Utc::now();
	let interval: Duration = ctx.args_sub.interval.into();
	let not_before = now - interval;
	info!(?now, ?not_before, "date range for alerts");

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

	// TODO: convert to join!
	for alert in alerts {
		if let Err(err) = execute_alert(
			&client,
			&config.mailgun,
			&alert,
			now,
			not_before,
			ctx.args_sub.dry_run,
		)
		.await.wrap_err(format!("while executing alert: {}", alert.file.display()))
		{
			eprintln!("{err:?}");
		}
	}

	Ok(())
}

#[instrument(skip(client, mailgun, alert, now, not_before))]
async fn execute_alert(
	client: &tokio_postgres::Client,
	mailgun: &TamanuMailgun,
	alert: &AlertDefinition,
	now: chrono::DateTime<chrono::Utc>,
	not_before: chrono::DateTime<chrono::Utc>,
	dry_run: bool,
) -> Result<()> {
	info!(?alert.file, "executing alert");

	let rows = client
		.query(&alert.sql, &[&not_before])
		.await
		.into_diagnostic()
		.wrap_err("querying database")?;

	if rows.is_empty() {
		debug!(?alert.file, "no rows returned, skipping");
		return Ok(());
	}
	info!(?alert.file, rows=%rows.len(), "alert triggered");

	debug!("compiling Tera templates");
	let mut tera = tera::Tera::default();
	tera.add_raw_template(
		"subject",
		alert.subject.as_deref().unwrap_or(DEFAULT_SUBJECT_TEMPLATE),
	)
	.into_diagnostic()
	.wrap_err("compiling subject template")?;
	tera.add_raw_template("alert.html", &alert.template)
		.into_diagnostic()
		.wrap_err("compiling email template")?;

	debug!("building Tera context");
	let context_rows = rows_to_value_map(&rows);

	let mut context = tera::Context::new();
	context.insert("rows", &context_rows);
	context.insert("interval", &format!("{:?}", now - not_before));
	context.insert(
		"hostname",
		System::host_name().as_deref().unwrap_or("unknown"),
	);
	context.insert(
		"filename",
		&alert.file.file_name().unwrap().to_string_lossy(),
	);
	context.insert("now", &now.to_string());

	debug!("rendering Tera templates");
	let subject = tera
		.render("subject", &context)
		.into_diagnostic()
		.wrap_err("rendering subject template")?;
	let body = tera
		.render("alert.html", &context)
		.into_diagnostic()
		.wrap_err("rendering email template")?;

	if dry_run {
		println!("-------------------------------");
		println!("Alert: {}", alert.file.display());
		println!("Subject: {subject}");
		println!("Body: {body}");
		return Ok(());
	}

	debug!(?alert.recipients, "sending email");
	let sender = EmailAddress::address(&mailgun.sender);
	let message = Mailgun {
		api_key: mailgun.api_key.clone(),
		domain: mailgun.domain.clone(),
		message: Message {
			to: alert
				.recipients
				.iter()
				.map(|email| EmailAddress::address(email))
				.collect(),
			subject,
			html: body,
			..Default::default()
		},
	};
	message
		.async_send(mailgun_rs::MailgunRegion::US, &sender)
		.await
		.into_diagnostic()
		.wrap_err("sending email")?;

	Ok(())
}
