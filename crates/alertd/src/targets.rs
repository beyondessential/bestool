use std::collections::HashMap;

use miette::Result;

use crate::{
	EmailConfig,
	alert::AlertDefinition,
	templates::{load_templates, render_alert},
};

mod email;

pub use email::TargetEmail;

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case", tag = "target")]
pub enum SendTarget {
	Email {
		subject: Option<String>,
		template: String,
		#[serde(flatten)]
		conn: TargetEmail,
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
					})
					.collect()
			}),
			_ => None,
		}
	}

	pub async fn send(
		&self,
		alert: &AlertDefinition,
		tera_ctx: &mut tera::Context,
		email: Option<&EmailConfig>,
		dry_run: bool,
	) -> Result<()> {
		let tera = load_templates(self)?;
		let (subject, body) = render_alert(&tera, tera_ctx)?;

		match self {
			SendTarget::Email { conn, .. } => {
				conn.send(alert, email, &subject, &body, dry_run).await?;
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
		conn: TargetEmail,
	},
}

impl ExternalTarget {
	pub fn id(&self) -> &str {
		match self {
			Self::Email { id, .. } => id,
		}
	}
}
