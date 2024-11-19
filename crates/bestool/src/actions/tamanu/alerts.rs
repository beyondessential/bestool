use std::{error::Error, ops::ControlFlow, path::PathBuf, process, time::Duration};

use bytes::{BufMut, BytesMut};
use chrono::{DateTime, Utc};
use clap::Parser;
use folktime::duration::{Duration as Folktime, Style as FolkStyle};
use mailgun_rs::{EmailAddress, Mailgun, Message};
use miette::{miette, Context as _, IntoDiagnostic, Result};
use reqwest::Url;
use serde_json::json;
use sysinfo::System;
use tera::{Context as TeraCtx, Tera};
use tokio::io::AsyncReadExt as _;
use tokio_postgres::types::{IsNull, ToSql, Type};
use tracing::{debug, error, info, instrument, warn};
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
/// which can have a binding for a datetime in the past to limit results by;
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
///
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
/// - `rows`: the result of the SQL query, as a list of objects
/// - `interval`: the duration string of the alert interval
/// - `hostname`: the hostname of the machine running this command
/// - `filename`: the name of the alert definition file
/// - `now`: the current date and time
///
/// Additionally you can `{% include "subject" %}` to include the rendering of
/// the subject template in the email template.
///
/// # Query binding parameters
///
/// The SQL query will be passed exactly the number of parameters it expects.
/// The parameters are always provided in this order:
///
/// $1: the datetime of the start of the interval (timestamp with time zone)
/// $2: the interval duration (interval)
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct AlertsArgs {
	/// Folder containing alert definitions.
	///
	/// This folder will be read recursively for files with the `.yaml` or `.yml` extension.
	///
	/// Files that don't match the expected format will be skipped, as will files with
	/// `enabled: false`.
	///
	/// Can be provided multiple times.
	#[arg(long)]
	pub dir: Vec<PathBuf>,

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
	#[serde(skip)]
	interval: Duration,
	#[serde(default)]
	send: Vec<SendTarget>,

	#[serde(flatten)]
	source: TicketSource,

	// legacy email-only fields
	#[serde(default)]
	recipients: Vec<String>,
	subject: Option<String>,
	template: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
#[serde(untagged, deny_unknown_fields)]
enum TicketSource {
	Sql { sql: String },
	Shell { shell: String, run: String },
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "snake_case", tag = "target")]
enum SendTarget {
	Email {
		addresses: Vec<String>,
		subject: Option<String>,
		template: String,
	},
	Zendesk {
		endpoint: Url,

		#[serde(flatten)]
		method: ZendeskMethod,

		subject: Option<String>,

		ticket_form_id: Option<u64>,
		#[serde(default)]
		custom_fields: Vec<ZendeskCustomField>,

		template: String,
	},
}

#[derive(serde::Deserialize, Debug)]
#[serde(untagged, deny_unknown_fields)]
enum ZendeskMethod {
	// Make credentials and requester fields exclusive as specifying the requester object in authorized
	// request is invalid. We may be able to specify some account as the requester, but it's not
	// necessary. That's because the requester defaults to the authenticated account.
	Authorized { credentials: ZendeskCredentials },
	Anonymous { requester: String },
}

#[derive(serde::Deserialize, Debug)]
struct ZendeskCredentials {
	email: String,
	password: String,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
struct ZendeskCustomField {
	id: u64,
	value: String,
}

impl AlertDefinition {
	fn normalise(mut self) -> Self {
		if !self.recipients.is_empty() {
			self.send.push(SendTarget::Email {
				addresses: self.recipients,
				subject: self.subject,
				template: self.template.unwrap_or_default(),
			});
			self.recipients = vec![];
			self.subject = None;
			self.template = None;
		}

		self
	}
}

struct InternalContext {
	pg_client: tokio_postgres::Client,
	http_client: reqwest::Client,
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

	let mut alerts = Vec::<AlertDefinition>::new();
	for dir in ctx.args_sub.dir {
		alerts.extend(
			WalkDir::new(dir)
				.into_iter()
				.filter_map(|e| e.ok())
				.filter(|e| e.file_type().is_file())
				.map(|entry| {
					let file = entry.path();
					if file
						.extension()
						.map_or(false, |e| e == "yaml" || e == "yml")
					{
						debug!(?file, "parsing YAML file");
						let content = std::fs::read_to_string(file)
							.into_diagnostic()
							.wrap_err(format!("{file:?}"))?;
						let mut alert: AlertDefinition = serde_yml::from_str(&content)
							.into_diagnostic()
							.wrap_err(format!("{file:?}"))?;

						alert.file = file.to_path_buf();
						alert.interval = ctx.args_sub.interval.into();
						let alert = alert.normalise();
						debug!(?alert, "parsed alert file");
						if alert.enabled {
							return Ok(Some(alert));
						}
					}

					Ok(None)
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

	let internal_ctx = InternalContext {
		pg_client: client,
		http_client: reqwest::Client::new(),
	};

	// TODO: convert to join!
	for alert in alerts {
		if let Err(err) =
			execute_alert(&internal_ctx, &config.mailgun, &alert, ctx.args_sub.dry_run)
				.await
				.wrap_err(format!("while executing alert: {}", alert.file.display()))
		{
			eprintln!("{err:?}");
		}
	}

	Ok(())
}

#[instrument]
fn load_templates(target: &SendTarget) -> Result<Tera> {
	let mut tera = tera::Tera::default();

	match target {
		SendTarget::Email {
			subject, template, ..
		}
		| SendTarget::Zendesk {
			subject, template, ..
		} => {
			tera.add_raw_template(
				"subject",
				subject.as_deref().unwrap_or(DEFAULT_SUBJECT_TEMPLATE),
			)
			.into_diagnostic()
			.wrap_err("compiling subject template")?;
			tera.add_raw_template("alert.html", &template)
				.into_diagnostic()
				.wrap_err("compiling email template")?;
		}
	}
	if let SendTarget::Zendesk {
		method: ZendeskMethod::Anonymous { requester },
		..
	} = target
	{
		tera.add_raw_template("requester", requester)
			.into_diagnostic()
			.wrap_err("compiling requester template")?;
	}
	Ok(tera)
}

#[instrument(skip(alert, now))]
fn build_context(alert: &AlertDefinition, now: chrono::DateTime<chrono::Utc>) -> TeraCtx {
	let mut context = TeraCtx::new();
	context.insert(
		"interval",
		&format!(
			"{}",
			Folktime::new(alert.interval).with_style(FolkStyle::OneUnitWhole)
		),
	);
	context.insert(
		"hostname",
		System::host_name().as_deref().unwrap_or("unknown"),
	);
	context.insert(
		"filename",
		&alert.file.file_name().unwrap().to_string_lossy(),
	);
	context.insert("now", &now.to_string());

	context
}

#[instrument(skip(client, alert, not_before, context))]
async fn read_sources(
	client: &tokio_postgres::Client,
	alert: &AlertDefinition,
	not_before: DateTime<Utc>,
	context: &mut TeraCtx,
) -> Result<ControlFlow<(), ()>> {
	match &alert.source {
		TicketSource::Sql { sql } => {
			let statement = client.prepare(sql).await.into_diagnostic()?;

			let interval = Interval(alert.interval);
			let all_params: Vec<&(dyn ToSql + Sync)> = vec![&not_before, &interval];

			let rows = client
				.query(&statement, &all_params[..statement.params().len()])
				.await
				.into_diagnostic()
				.wrap_err("querying database")?;

			if rows.is_empty() {
				debug!(?alert.file, "no rows returned, skipping");
				return Ok(ControlFlow::Break(()));
			}
			info!(?alert.file, rows=%rows.len(), "alert triggered");

			let context_rows = rows_to_value_map(&rows);

			context.insert("rows", &context_rows);
		}
		TicketSource::Shell { shell, run } => {
			let mut shell = tokio::process::Command::new(shell)
				.arg("-c") // "-c" for "command" is in the POSIX standard and well supported incl. PowerShell 7.
				.arg(run)
				.stdin(process::Stdio::null())
				.stdout(process::Stdio::piped())
				.spawn()
				.into_diagnostic()?;

			let mut output = Vec::new();
			let mut stdout = shell
				.stdout
				.take()
				.ok_or_else(|| miette!("getting the child stdout handle"))?;
			let output_future =
				futures::future::try_join(shell.wait(), stdout.read_to_end(&mut output));

			let Ok(res) = tokio::time::timeout(alert.interval, output_future).await else {
				warn!(?alert.file, "the script timed out, skipping");
				shell.kill().await.into_diagnostic()?;
				return Ok(ControlFlow::Break(()));
			};

			let (status, output_size) = res.into_diagnostic().wrap_err("running the shell")?;

			if status.success() {
				debug!(?alert.file, "the script succeeded, skipping");
				return Ok(ControlFlow::Break(()));
			}
			info!(?alert.file, ?status, ?output_size, "alert triggered");

			context.insert("output", &String::from_utf8_lossy(&output));
		}
	}
	Ok(ControlFlow::Continue(()))
}

#[instrument(skip(tera, context))]
fn render_alert(tera: &Tera, context: &mut TeraCtx) -> Result<(String, String, Option<String>)> {
	let subject = tera
		.render("subject", &context)
		.into_diagnostic()
		.wrap_err("rendering subject template")?;

	context.insert("subject", &subject.to_string());

	let body = tera
		.render("alert.html", &context)
		.into_diagnostic()
		.wrap_err("rendering email template")?;

	let requester = tera
		.render("requester", &context)
		.map(Some)
		.or_else(|err| match err.kind {
			tera::ErrorKind::TemplateNotFound(_) => Ok(None),
			_ => Err(err),
		})
		.into_diagnostic()
		.wrap_err("rendering requester template")?;

	Ok((subject, body, requester))
}

#[instrument(skip(ctx, mailgun, alert))]
async fn execute_alert(
	ctx: &InternalContext,
	mailgun: &TamanuMailgun,
	alert: &AlertDefinition,
	dry_run: bool,
) -> Result<()> {
	info!(?alert.file, "executing alert");

	let now = chrono::Utc::now();
	let not_before = now - alert.interval;
	info!(?now, ?not_before, interval=?alert.interval, "date range for alert");

	let mut tera_ctx = build_context(alert, now);
	if read_sources(&ctx.pg_client, alert, not_before, &mut tera_ctx)
		.await?
		.is_break()
	{
		return Ok(());
	}

	for target in &alert.send {
		let tera = load_templates(target)?;
		let (subject, body, requester) = render_alert(&tera, &mut tera_ctx)?;
		match target {
			SendTarget::Email { addresses, .. } => {
				if dry_run {
					println!("-------------------------------");
					println!("Alert: {}", alert.file.display());
					println!("Recipients: {}", addresses.join(", "));
					println!("Subject: {subject}");
					println!("Body: {body}");
					continue;
				}

				debug!(?alert.recipients, "sending email");
				let sender = EmailAddress::address(&mailgun.sender);
				let message = Mailgun {
					api_key: mailgun.api_key.clone(),
					domain: mailgun.domain.clone(),
					message: Message {
						to: addresses
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
			}
			SendTarget::Zendesk {
				endpoint,
				method,
				ticket_form_id,
				custom_fields,
				..
			} => {
				if dry_run {
					println!("-------------------------------");
					println!("Alert: {}", alert.file.display());
					println!("Endpoint: {}", endpoint);
					println!("Subject: {subject}");
					println!("Body: {body}");
					continue;
				}

				let req = json!({
					"request": {
						"subject": subject,
						"ticket_form_id": ticket_form_id,
						"custom_fields": custom_fields,
						"comment": { "html_body": body },
						"requester": requester.map(|r| json!({ "name": r }))
					}
				});

				let mut req_builder = ctx.http_client.post(endpoint.clone()).json(&req);

				if let ZendeskMethod::Authorized {
					credentials: ZendeskCredentials { email, password },
				} = method
				{
					req_builder =
						req_builder.basic_auth(std::format_args!("{email}/token"), Some(password));
				}

				let resp = req_builder
					.send()
					.await
					.into_diagnostic()
					.wrap_err("creating Zendesk ticket")?;
				debug!(resp_text = ?resp.text().await.into_diagnostic()?, "Zendesk ticket sent");
			}
		}
	}

	Ok(())
}

#[derive(Debug)]
struct Interval(pub Duration);

impl ToSql for Interval {
	fn to_sql(&self, _: &Type, out: &mut BytesMut) -> Result<IsNull, Box<dyn Error + Sync + Send>> {
		out.put_i64(self.0.as_micros().try_into().unwrap_or_default());
		out.put_i32(0);
		out.put_i32(0);
		Ok(IsNull::No)
	}

	fn accepts(ty: &Type) -> bool {
		matches!(*ty, Type::INTERVAL)
	}

	tokio_postgres::types::to_sql_checked!();
}

#[cfg(test)]
mod tests {
	use chrono::{Duration, Utc};

	use super::*;

	fn interval_context(dur: Duration) -> Option<String> {
		let alert = AlertDefinition {
			file: PathBuf::from("test.yaml"),
			enabled: true,
			interval: dur.to_std().unwrap(),
			source: TicketSource::Sql { sql: "".into() },
			send: vec![],
			recipients: vec![],
			subject: None,
			template: None,
		};
		build_context(&alert, Utc::now())
			.get("interval")
			.and_then(|v| v.as_str())
			.map(|s| s.to_owned())
	}

	#[test]
	fn test_interval_format_minutes() {
		assert_eq!(
			interval_context(Duration::minutes(15)).as_deref(),
			Some("15m"),
		);
	}

	#[test]
	fn test_interval_format_hour() {
		assert_eq!(interval_context(Duration::hours(1)).as_deref(), Some("1h"),);
	}

	#[test]
	fn test_interval_format_day() {
		assert_eq!(interval_context(Duration::days(1)).as_deref(), Some("1d"),);
	}

	#[test]
	fn test_alert_parse_email() {
		let alert = r#"
sql: SELECT $1::timestamptz;
send:
- target: email
  addresses: [test@example.com]
  subject: "[Tamanu Alert] Example ({{ hostname }})"
  template: |
    <p>Server: {{ hostname }}</p>
    <p>There are {{ rows | length }} rows.</p>
"#;
		let alert: AlertDefinition = serde_yml::from_str(&alert).unwrap();
		let alert = alert.normalise();
		assert_eq!(alert.interval, std::time::Duration::default());
		assert!(
			matches!(alert.source, TicketSource::Sql { sql } if sql == "SELECT $1::timestamptz;")
		);
		assert!(matches!(alert.send[0], SendTarget::Email { .. }));
	}

	#[test]
	fn test_alert_parse_shell() {
		let alert = r#"
shell: bash
run: echo foobar
"#;
		let alert: AlertDefinition = serde_yml::from_str(&alert).unwrap();
		let alert = alert.normalise();
		assert_eq!(alert.interval, std::time::Duration::default());
		assert!(
			matches!(alert.source, TicketSource::Shell { shell, run } if shell == "bash" && run == "echo foobar")
		);
	}

	#[test]
	fn test_alert_parse_invalid_source() {
		let alert = r#"
shell: bash
"#;
		assert!(matches!(
			serde_yml::from_str::<AlertDefinition>(&alert),
			Err(_)
		));
		let alert = r#"
run: echo foo
"#;
		assert!(matches!(
			serde_yml::from_str::<AlertDefinition>(&alert),
			Err(_)
		));
		let alert = r#"
sql: SELECT $1::timestamptz;
run: echo foo
"#;
		assert!(matches!(
			serde_yml::from_str::<AlertDefinition>(&alert),
			Err(_)
		));
		let alert = r#"
sql: SELECT $1::timestamptz;
shell: bash
"#;
		assert!(matches!(
			serde_yml::from_str::<AlertDefinition>(&alert),
			Err(_)
		));
		let alert = r#"
sql: SELECT $1::timestamptz;
shell: bash
run: echo foo
"#;
		assert!(matches!(
			serde_yml::from_str::<AlertDefinition>(&alert),
			Err(_)
		));
	}

	#[test]
	fn test_alert_parse_zendesk_authorized() {
		let alert = r#"
sql: SELECT $1::timestamptz;
send:
- target: zendesk
  endpoint: https://example.zendesk.com/api/v2/requests
  credentials:
    email: foo@example.com
    password: pass
  subject: "[Tamanu Alert] Example ({{ hostname }})"
  template: "Output: {{ output }}""#;
		let alert: AlertDefinition = serde_yml::from_str(&alert).unwrap();
		assert!(matches!(alert.send[0], SendTarget::Zendesk { .. }));
	}

	#[test]
	fn test_alert_parse_zendesk_anon() {
		let alert = r#"
sql: SELECT $1::timestamptz;
send:
- target: zendesk
  endpoint: https://example.zendesk.com/api/v2/requests
  requester: "{{ hostname }}"
  subject: "[Tamanu Alert] Example ({{ hostname }})"
  template: "Output: {{ output }}""#;
		let alert: AlertDefinition = serde_yml::from_str(&alert).unwrap();
		assert!(matches!(alert.send[0], SendTarget::Zendesk { .. }));
	}

	#[test]
	fn test_alert_parse_zendesk_form_fields() {
		let alert = r#"
sql: SELECT $1::timestamptz;
send:
- target: zendesk
  endpoint: https://example.zendesk.com/api/v2/requests
  requester: "{{ hostname }}"
  subject: "[Tamanu Alert] Example ({{ hostname }})"
  template: "Output: {{ output }}"
  ticket_form_id: 500
  custom_fields:
  - id: 100
    value: tamanu_
  - id: 200
    value: Test
"#;
		let alert: AlertDefinition = serde_yml::from_str(&alert).unwrap();
		assert!(matches!(alert.send[0], SendTarget::Zendesk { .. }));
	}

	#[test]
	fn test_alert_parse_legacy_recipients() {
		let alert = r#"
sql: |
  SELECT $1::timestamptz;
recipients:
  - test@example.com
subject: "[Tamanu Alert] Example ({{ hostname }})"
template: |
  <p>Server: {{ hostname }}</p>
  <p>There are {{ rows | length }} rows.</p>
"#;
		let alert: AlertDefinition = serde_yml::from_str(&alert).unwrap();
		let alert = alert.normalise();
		assert_eq!(alert.interval, std::time::Duration::default());
		assert!(matches!(alert.send[0], SendTarget::Email { .. }));
	}
}
