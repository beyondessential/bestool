use std::{
	collections::HashMap, error::Error, io::Write, ops::ControlFlow, path::PathBuf, process,
	time::Duration,
};

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

use super::{config::load_config, find_package, find_tamanu, TamanuArgs};

const DEFAULT_SUBJECT_TEMPLATE: &str = "[Tamanu Alert] {{ filename }} ({{ hostname }})";

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
/// Templates are rendered with [Tera](https://keats.github.io/tera/docs/#templates).
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
/// targets are: `email`, `zendesk`. Note that you can have multiple targets of
/// the same type.
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
///       <h1>Whoops</h1>
/// ```
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
	/// Can be provided multiple times.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--dir PATH`"))]
	#[arg(long)]
	pub dir: Vec<PathBuf>,

	/// How far back to look for alerts.
	///
	/// This is a duration string, e.g. `1d` for one day, `1h` for one hour, etc. It should match
	/// the task scheduling / cron interval for this command.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--interval DURATION`"))]
	#[arg(long)]
	pub interval: humantime::Duration,

	/// Don't actually send alerts, just print them to stdout.
	#[cfg_attr(docsrs, doc("\n\n**Flag**: `--dry-run`"))]
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

#[derive(serde::Deserialize, Debug, Default)]
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

#[derive(serde::Deserialize, Debug, Default)]
#[serde(untagged, deny_unknown_fields)]
enum TicketSource {
	Sql {
		sql: String,
	},
	Shell {
		shell: String,
		run: String,
	},

	#[default]
	None,
}

#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "snake_case", tag = "target")]
enum SendTarget {
	Email {
		subject: Option<String>,
		template: String,
		#[serde(flatten)]
		conn: TargetEmail,
	},
	Zendesk {
		subject: Option<String>,
		template: String,
		#[serde(flatten)]
		conn: TargetZendesk,
	},
	External {
		subject: Option<String>,
		template: String,
		id: String,
		#[serde(default, skip)]
		resolved: Option<ExternalTarget>,
	},
}

impl SendTarget {
	fn resolve_external(&mut self, external_targets: &HashMap<String, ExternalTarget>) {
		match self {
			Self::External { id, resolved, .. } => {
				if let Some(target) = external_targets.get(id) {
					*resolved = Some(target.clone());
				}
			}
			_ => {}
		}
	}
}

#[derive(serde::Deserialize, Debug)]
struct AlertTargets {
	targets: Vec<ExternalTarget>,
}

impl AlertTargets {
	fn to_map(self) -> HashMap<String, ExternalTarget> {
		self.targets
			.into_iter()
			.map(|target| (target.id().into(), target))
			.collect()
	}
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case", tag = "target")]
enum ExternalTarget {
	Email {
		id: String,
		#[serde(flatten)]
		conn: TargetEmail,
	},
	Zendesk {
		id: String,
		#[serde(flatten)]
		conn: TargetZendesk,
	},
}

impl ExternalTarget {
	fn id(&self) -> &str {
		match self {
			Self::Email { id, .. } => id,
			Self::Zendesk { id, .. } => id,
		}
	}
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
struct TargetEmail {
	addresses: Vec<String>,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
struct TargetZendesk {
	endpoint: Url,

	#[serde(flatten)]
	method: ZendeskMethod,

	ticket_form_id: Option<u64>,
	#[serde(default)]
	custom_fields: Vec<ZendeskCustomField>,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(untagged, deny_unknown_fields)]
enum ZendeskMethod {
	// Make credentials and requester fields exclusive as specifying the requester object in authorized
	// request is invalid. We may be able to specify some account as the requester, but it's not
	// necessary. That's because the requester defaults to the authenticated account.
	Authorized { credentials: ZendeskCredentials },
	Anonymous { requester: String },
}

#[derive(serde::Deserialize, Clone, Debug)]
struct ZendeskCredentials {
	email: String,
	password: String,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
struct ZendeskCustomField {
	id: u64,
	value: String,
}

impl AlertDefinition {
	fn normalise(mut self, external_targets: &HashMap<String, ExternalTarget>) -> Self {
		if !self.recipients.is_empty() {
			self.send.push(SendTarget::Email {
				subject: self.subject,
				template: self.template.unwrap_or_default(),
				conn: TargetEmail {
					addresses: self.recipients,
				},
			});
			self.recipients = vec![];
			self.subject = None;
			self.template = None;
		}

		for target in &mut self.send {
			target.resolve_external(external_targets);
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

	let config_value = load_config(&root, kind.package_name())?;
	let config: TamanuConfig = serde_json::from_value(config_value)
		.into_diagnostic()
		.wrap_err("parsing of Tamanu config failed")?;
	debug!(?config, "parsed Tamanu config");

	let mut alerts = Vec::<AlertDefinition>::new();
	let mut external_targets = HashMap::new();
	for dir in ctx.args_sub.dir {
		let external_targets_path = dir.join("_targets.yml");
		if let Some(target) = std::fs::read_to_string(&external_targets_path)
			.ok()
			.and_then(|content| {
				debug!(path=?external_targets_path, "parsing external targets");
				serde_yml::from_str::<AlertTargets>(&content)
					.map_err(
						|err| warn!(path=?external_targets_path, "_targets.yml has errors! {err}"),
					)
					.ok()
			}) {
			external_targets.extend(target.to_map().into_iter());
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

	let internal_ctx = InternalContext {
		pg_client: client,
		http_client: reqwest::Client::new(),
	};

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
		}
		| SendTarget::External {
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
		conn: TargetZendesk {
			method: ZendeskMethod::Anonymous { requester },
			..
		},
		..
	}
	| SendTarget::External {
		resolved:
			Some(ExternalTarget::Zendesk {
				conn:
					TargetZendesk {
						method: ZendeskMethod::Anonymous { requester },
						..
					},
				..
			}),
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
		TicketSource::None => {
			debug!(?alert.file, "no source, skipping");
			return Ok(ControlFlow::Break(()));
		}
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
			let mut shell = {
				let mut script = tempfile::Builder::new().tempfile().into_diagnostic()?;
				write!(script.as_file_mut(), "{run}").into_diagnostic()?;

				tokio::process::Command::new(shell)
					.arg(script.path())
					.stdin(process::Stdio::null())
					.stdout(process::Stdio::piped())
					.spawn()
					.into_diagnostic()?
			};

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
			SendTarget::Email {
				conn: TargetEmail { addresses },
				..
			}
			| SendTarget::External {
				resolved:
					Some(ExternalTarget::Email {
						conn: TargetEmail { addresses },
						..
					}),
				..
			} => {
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
				conn:
					TargetZendesk {
						endpoint,
						method,
						ticket_form_id,
						custom_fields,
					},
				..
			}
			| SendTarget::External {
				resolved:
					Some(ExternalTarget::Zendesk {
						conn:
							TargetZendesk {
								endpoint,
								method,
								ticket_form_id,
								custom_fields,
							},
						..
					}),
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

			SendTarget::External {
				resolved: None, id, ..
			} => {
				error!(?id, "external send target not found");
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
		let alert = alert.normalise(&Default::default());
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
		let alert = alert.normalise(&Default::default());
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
		let alert = alert.normalise(&Default::default());
		assert_eq!(alert.interval, std::time::Duration::default());
		assert!(matches!(alert.send[0], SendTarget::Email { .. }));
	}
}
