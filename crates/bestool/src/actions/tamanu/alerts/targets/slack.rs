use std::collections::HashMap;

use miette::{IntoDiagnostic, Result, WrapErr};
use reqwest::Url;
use tera::Tera;
use tracing::debug;

use crate::actions::tamanu::alerts::{
	InternalContext, definition::AlertDefinition, templates::TemplateField,
};

/// Parameters for sending a Slack alert
pub struct SlackSendParams<'a> {
	pub alert: &'a AlertDefinition,
	pub ctx: &'a InternalContext,
	pub subject: &'a str,
	pub body: &'a str,
	pub tera: &'a Tera,
	pub tera_ctx: &'a tera::Context,
	pub dry_run: bool,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct TargetSlack {
	pub webhook: Url,

	#[serde(default = "SlackField::default_set")]
	pub fields: Vec<SlackField>,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(untagged, rename_all = "snake_case")]
pub enum SlackField {
	Fixed { name: String, value: String },
	Field { name: String, field: TemplateField },
}

impl SlackField {
	pub fn default_set() -> Vec<Self> {
		vec![
			Self::Field {
				name: "hostname".into(),
				field: TemplateField::Hostname,
			},
			Self::Field {
				name: "filename".into(),
				field: TemplateField::Filename,
			},
			Self::Field {
				name: "subject".into(),
				field: TemplateField::Subject,
			},
			Self::Field {
				name: "message".into(),
				field: TemplateField::Body,
			},
		]
	}
}

impl TargetSlack {
	pub async fn send(&self, params: SlackSendParams<'_>) -> Result<()> {
		let SlackSendParams {
			alert,
			ctx,
			subject,
			body,
			tera,
			tera_ctx,
			dry_run,
		} = params;
		if dry_run {
			println!("-------------------------------");
			println!("Alert: {}", alert.file.display());
			println!("Recipients: slack");
			println!("Subject: {subject}");
			println!("Body: {body}");
			return Ok(());
		}

		let payload: HashMap<&String, String> = self
			.fields
			.iter()
			.map(|field| match field {
				SlackField::Fixed { name, value } => (name, value.clone()),
				SlackField::Field { name, field } => (
					name,
					tera.render(field.as_str(), tera_ctx)
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

		debug!(?self.webhook, ?payload, "posting to slack webhook");
		ctx.http_client
			.post(self.webhook.clone())
			.json(&payload)
			.send()
			.await
			.into_diagnostic()
			.wrap_err("posting to slack webhook")
			.map(drop)
	}
}
