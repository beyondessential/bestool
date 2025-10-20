use std::{collections::HashMap, sync::Arc};

use miette::Result;

use crate::actions::tamanu::config::TamanuConfig;

use super::{
	definition::AlertDefinition,
	templates::{load_templates, render_alert},
	InternalContext,
};

mod email;
mod slack;
pub(super) mod zendesk;

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case", tag = "target")]
pub enum SendTarget {
	Email {
		subject: Option<String>,
		template: String,
		#[serde(flatten)]
		conn: email::TargetEmail,
	},
	Zendesk {
		subject: Option<String>,
		template: String,
		#[serde(flatten)]
		conn: zendesk::TargetZendesk,
	},
	Slack {
		subject: Option<String>,
		template: String,
		#[serde(flatten)]
		conn: slack::TargetSlack,
	},
	External {
		subject: Option<String>,
		template: String,
		id: String,
	},
}

impl SendTarget {
	pub fn resolve_external(
		&self,
		external_targets: &HashMap<String, Vec<ExternalTarget>>,
	) -> Option<Vec<SendTarget>> {
		match self {
			Self::External {
				id,
				subject,
				template,
			} => external_targets.get(id).map(|exts| {
				exts.iter()
					.map(|ext| match ext {
						ExternalTarget::Email { conn, .. } => SendTarget::Email {
							subject: subject.clone(),
							template: template.clone(),
							conn: conn.clone(),
						},
						ExternalTarget::Zendesk { conn, .. } => SendTarget::Zendesk {
							subject: subject.clone(),
							template: template.clone(),
							conn: conn.clone(),
						},
						ExternalTarget::Slack { conn, .. } => SendTarget::Slack {
							subject: subject.clone(),
							template: template.clone(),
							conn: conn.clone(),
						},
					})
					.collect()
			}),
			_ => None,
		}
	}

	pub async fn send(
		&self,
		alert: &AlertDefinition,
		ctx: Arc<InternalContext>,
		tera_ctx: &mut tera::Context,
		config: &TamanuConfig,
		dry_run: bool,
	) -> Result<()> {
		let tera = load_templates(self)?;
		let (subject, body, requester) = render_alert(&tera, tera_ctx)?;

		match self {
			SendTarget::Email { conn, .. } => {
				conn.send(alert, config, &subject, &body, dry_run).await?;
			}

			SendTarget::Slack { conn, .. } => {
				conn.send(alert, &ctx, &subject, &body, &tera, tera_ctx, dry_run)
					.await?;
			}

			SendTarget::Zendesk { conn, .. } => {
				conn.send(alert, &ctx, &subject, &body, requester.as_deref(), dry_run)
					.await?;
			}

			SendTarget::External { .. } => {
				unreachable!("external targets should be resolved before here");
			}
		}

		Ok(())
	}
}

#[derive(serde::Deserialize, Debug)]
pub struct AlertTargets {
	pub targets: Vec<ExternalTarget>,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case", tag = "target")]
pub enum ExternalTarget {
	Email {
		id: String,
		#[serde(flatten)]
		conn: email::TargetEmail,
	},
	Zendesk {
		id: String,
		#[serde(flatten)]
		conn: zendesk::TargetZendesk,
	},
	Slack {
		id: String,
		#[serde(flatten)]
		conn: slack::TargetSlack,
	},
}

impl ExternalTarget {
	pub fn id(&self) -> &str {
		match self {
			Self::Email { id, .. } => id,
			Self::Zendesk { id, .. } => id,
			Self::Slack { id, .. } => id,
		}
	}
}
