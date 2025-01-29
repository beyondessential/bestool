use std::{
	collections::HashMap, io::Write, ops::ControlFlow, path::PathBuf, process::Stdio, sync::Arc,
	time::Duration,
};

use chrono::{DateTime, Utc};
use mailgun_rs::{EmailAddress, Mailgun, Message};
use miette::{miette, Context as _, IntoDiagnostic, Result};
use serde_json::json;
use tera::Context as TeraCtx;
use tokio::io::AsyncReadExt as _;
use tokio_postgres::types::ToSql;
use tracing::{debug, error, info, instrument, warn};

use crate::{actions::tamanu::config, postgres_to_value::rows_to_value_map};

use super::{
	pg_interval::Interval,
	targets::{ExternalTarget, SendTarget, TargetEmail},
	targets::{SlackField, TargetSlack, TargetZendesk, ZendeskCredentials, ZendeskMethod},
	templates::build_context,
	templates::{load_templates, render_alert},
	InternalContext,
};

fn enabled() -> bool {
	true
}

#[derive(serde::Deserialize, Debug, Default)]
pub struct AlertDefinition {
	#[serde(default, skip)]
	pub file: PathBuf,

	#[serde(default = "enabled")]
	pub enabled: bool,
	#[serde(skip)]
	pub interval: Duration,
	#[serde(default)]
	pub send: Vec<SendTarget>,

	#[serde(flatten)]
	pub source: TicketSource,

	// legacy email-only fields
	#[serde(default)]
	pub recipients: Vec<String>,
	pub subject: Option<String>,
	pub template: Option<String>,
}

#[derive(serde::Deserialize, Debug, Default)]
#[serde(untagged, deny_unknown_fields)]
pub enum TicketSource {
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

impl AlertDefinition {
	pub fn normalise(mut self, external_targets: &HashMap<String, Vec<ExternalTarget>>) -> Self {
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

		self.send = self
			.send
			.iter()
			.flat_map(|target| match target {
				target @ SendTarget::External { id, .. } => target
					.resolve_external(external_targets)
					.unwrap_or_else(|| {
						error!(id, "external target not found");
						Vec::new()
					}),
				other => vec![other.clone()],
			})
			.collect();

		self
	}

	#[instrument(skip(self, client, not_before, context))]
	pub async fn read_sources(
		&self,
		client: &tokio_postgres::Client,
		not_before: DateTime<Utc>,
		context: &mut TeraCtx,
	) -> Result<ControlFlow<(), ()>> {
		match &self.source {
			TicketSource::None => {
				debug!(?self.file, "no source, skipping");
				return Ok(ControlFlow::Break(()));
			}
			TicketSource::Sql { sql } => {
				let statement = client.prepare(sql).await.into_diagnostic()?;

				let interval = Interval(self.interval);
				let all_params: Vec<&(dyn ToSql + Sync)> = vec![&not_before, &interval];

				let rows = client
					.query(&statement, &all_params[..statement.params().len()])
					.await
					.into_diagnostic()
					.wrap_err("querying database")?;

				if rows.is_empty() {
					debug!(?self.file, "no rows returned, skipping");
					return Ok(ControlFlow::Break(()));
				}
				info!(?self.file, rows=%rows.len(), "alert triggered");

				let context_rows = rows_to_value_map(&rows);

				context.insert("rows", &context_rows);
			}
			TicketSource::Shell { shell, run } => {
				let mut script = tempfile::Builder::new().tempfile().into_diagnostic()?;
				write!(script.as_file_mut(), "{run}").into_diagnostic()?;

				let mut shell = tokio::process::Command::new(shell)
					.arg(script.path())
					.stdin(Stdio::null())
					.stdout(Stdio::piped())
					.spawn()
					.into_diagnostic()?;

				let mut output = Vec::new();
				let mut stdout = shell
					.stdout
					.take()
					.ok_or_else(|| miette!("getting the child stdout handle"))?;
				let output_future =
					futures::future::try_join(shell.wait(), stdout.read_to_end(&mut output));

				let Ok(res) = tokio::time::timeout(self.interval, output_future).await else {
					warn!(?self.file, "the script timed out, skipping");
					shell.kill().await.into_diagnostic()?;
					return Ok(ControlFlow::Break(()));
				};

				let (status, output_size) = res.into_diagnostic().wrap_err("running the shell")?;

				if status.success() {
					debug!(?self.file, "the script succeeded, skipping");
					return Ok(ControlFlow::Break(()));
				}
				info!(?self.file, ?status, ?output_size, "alert triggered");

				context.insert("output", &String::from_utf8_lossy(&output));
			}
		}
		Ok(ControlFlow::Continue(()))
	}

	pub async fn execute(
		self,
		ctx: Arc<InternalContext>,
		mailgun: Arc<config::Mailgun>,
		dry_run: bool,
	) -> Result<()> {
		info!(?self.file, "executing alert");

		let now = crate::now_time(&chrono::Utc);
		let not_before = now - self.interval;
		info!(?now, ?not_before, interval=?self.interval, "date range for alert");

		let mut tera_ctx = build_context(&self, now);
		if self
			.read_sources(&ctx.pg_client, not_before, &mut tera_ctx)
			.await?
			.is_break()
		{
			return Ok(());
		}

		for target in &self.send {
			let tera = load_templates(target)?;
			let (subject, body, requester) = render_alert(&tera, &mut tera_ctx)?;

			match target {
				SendTarget::Email {
					conn: TargetEmail { addresses },
					..
				} => {
					if dry_run {
						println!("-------------------------------");
						println!("Alert: {}", self.file.display());
						println!("Recipients: {}", addresses.join(", "));
						println!("Subject: {subject}");
						println!("Body: {body}");
						continue;
					}

					debug!(?self.recipients, "sending email");
					let sender = EmailAddress::address(&mailgun.sender);
					let mailgun = Mailgun {
						api_key: mailgun.api_key.clone(),
						domain: mailgun.domain.clone(),
					};
					let message = Message {
						to: addresses
							.iter()
							.map(|email| EmailAddress::address(email))
							.collect(),
						subject,
						html: body,
						..Default::default()
					};
					mailgun
						.async_send(mailgun_rs::MailgunRegion::US, &sender, message)
						.await
						.into_diagnostic()
						.wrap_err("sending email")?;
				}

				SendTarget::Slack {
					conn: TargetSlack { webhook, fields },
					..
				} => {
					if dry_run {
						println!("-------------------------------");
						println!("Alert: {}", self.file.display());
						println!("Recipients: slack");
						println!("Subject: {subject}");
						println!("Body: {body}");
						continue;
					}

					let payload: HashMap<&String, String> = fields
						.iter()
						.map(|field| match field {
							SlackField::Fixed { name, value } => (name, value.clone()),
							SlackField::Field { name, field } => (
								name,
								tera.render(field.as_str(), &tera_ctx)
									.ok()
									.or_else(|| {
										tera_ctx.get(field.as_str()).map(|v| match v.as_str() {
											Some(t) => t.to_owned(),
											None => v.to_string(),
										})
									})
									.unwrap_or_default(),
							),
						})
						.collect();

					debug!(?webhook, ?payload, "posting to slack webhook");
					ctx.http_client
						.post(webhook.clone())
						.json(&payload)
						.send()
						.await
						.into_diagnostic()
						.wrap_err("posting to slack webhook")?;
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
				} => {
					if dry_run {
						println!("-------------------------------");
						println!("Alert: {}", self.file.display());
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
						req_builder = req_builder
							.basic_auth(std::format_args!("{email}/token"), Some(password));
					}

					req_builder
						.send()
						.await
						.into_diagnostic()
						.wrap_err("creating Zendesk ticket")?;
					debug!("Zendesk ticket sent");
				}

				SendTarget::External { .. } => {
					unreachable!("external targets should be resolved before here");
				}
			}
		}

		Ok(())
	}
}
