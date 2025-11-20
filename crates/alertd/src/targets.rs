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
#[serde(rename_all = "snake_case")]
#[serde(untagged)]
pub enum SendTarget {
	// New format: just id, subject, template
	Simple {
		id: String,
		subject: Option<String>,
		template: String,
	},
	// Old format: target: external, id, subject, template
	External {
		target: String, // Should be "external" but we ignore the value
		id: String,
		subject: Option<String>,
		template: String,
	},
}

impl SendTarget {
	pub fn id(&self) -> &str {
		match self {
			Self::Simple { id, .. } => id,
			Self::External { id, .. } => id,
		}
	}

	pub fn subject(&self) -> &Option<String> {
		match self {
			Self::Simple { subject, .. } => subject,
			Self::External { subject, .. } => subject,
		}
	}

	pub fn template(&self) -> &str {
		match self {
			Self::Simple { template, .. } => template,
			Self::External { template, .. } => template,
		}
	}

	pub fn resolve_external(
		&self,
		external_targets: &HashMap<String, Vec<ExternalTarget>>,
	) -> Vec<ResolvedTarget> {
		external_targets
			.get(self.id())
			.map(|exts| {
				exts.iter()
					.map(|ext| ResolvedTarget {
						subject: self.subject().clone(),
						template: self.template().to_string(),
						conn: ext.conn.clone(),
					})
					.collect()
			})
			.unwrap_or_default()
	}
}

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
	pub subject: Option<String>,
	pub template: String,
	pub conn: TargetEmail,
}

impl ResolvedTarget {
	pub async fn send(
		&self,
		alert: &AlertDefinition,
		tera_ctx: &mut tera::Context,
		email: Option<&EmailConfig>,
		dry_run: bool,
	) -> Result<()> {
		let tera = load_templates(&self.subject, &self.template)?;
		let (subject, body) = render_alert(&tera, tera_ctx)?;

		self.conn.send(alert, email, &subject, &body, dry_run).await
	}
}

#[derive(serde::Deserialize, Debug)]
pub struct AlertTargets {
	pub targets: Vec<ExternalTarget>,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct ExternalTarget {
	pub id: String,
	#[serde(flatten)]
	pub conn: TargetEmail,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_send_target_simple_format() {
		let yaml = r#"
id: test-target
subject: Test Subject
template: Test template
"#;
		let target: SendTarget = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(target.id(), "test-target");
		assert_eq!(target.subject(), &Some("Test Subject".to_string()));
		assert_eq!(target.template(), "Test template");
	}

	#[test]
	fn test_send_target_external_format() {
		let yaml = r#"
target: external
id: test-target
subject: Test Subject
template: Test template
"#;
		let target: SendTarget = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(target.id(), "test-target");
		assert_eq!(target.subject(), &Some("Test Subject".to_string()));
		assert_eq!(target.template(), "Test template");
	}

	#[test]
	fn test_send_target_without_subject() {
		let yaml = r#"
id: test-target
template: Test template
"#;
		let target: SendTarget = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(target.id(), "test-target");
		assert_eq!(target.subject(), &None);
		assert_eq!(target.template(), "Test template");
	}
}
