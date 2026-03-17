use std::collections::HashMap;

use miette::Result;

use crate::{
	EmailConfig,
	alert::AlertDefinition,
	templates::{load_templates, render_alert},
};

mod default;
mod email;
mod slack;

pub use default::determine_default_target;
pub use email::TargetEmail;
pub use slack::TargetSlack;

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

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum TargetConnection {
	Slack(TargetSlack),
	Email(TargetEmail),
}

#[derive(Debug, Clone)]
pub struct ResolvedTarget {
	pub subject: Option<String>,
	pub template: String,
	pub conn: TargetConnection,
}

impl ResolvedTarget {
	pub async fn send(
		&self,
		alert: &AlertDefinition,
		tera_ctx: &mut tera::Context,
		email: Option<&EmailConfig>,
		http_client: Option<&reqwest::Client>,
		dry_run: bool,
	) -> Result<()> {
		let tera = load_templates(&self.subject, &self.template)?;
		let (subject, body) = render_alert(&tera, tera_ctx)?;

		match &self.conn {
			TargetConnection::Email(target) => {
				target.send(alert, email, &subject, &body, dry_run).await
			}
			TargetConnection::Slack(target) => {
				let client = http_client.ok_or_else(|| {
					miette::miette!("slack target requires an HTTP client (this is a bug)")
				})?;
				target
					.send(
						client,
						slack::SlackSendParams {
							alert,
							subject: &subject,
							body: &body,
							tera: &tera,
							tera_ctx,
							dry_run,
						},
					)
					.await
			}
		}
	}
}

#[derive(serde::Deserialize, Debug)]
pub struct AlertTargets {
	pub targets: Vec<ExternalTarget>,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct ExternalTarget {
	pub id: String,
	#[serde(flatten)]
	pub conn: TargetConnection,
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

	#[test]
	fn test_external_target_email() {
		let yaml = r#"
id: ops-team
addresses:
  - ops@example.com
  - oncall@example.com
"#;
		let target: ExternalTarget = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(target.id, "ops-team");
		assert!(matches!(target.conn, TargetConnection::Email(_)));
		if let TargetConnection::Email(email) = &target.conn {
			assert_eq!(email.addresses.len(), 2);
			assert_eq!(email.addresses[0], "ops@example.com");
		}
	}

	#[test]
	fn test_external_target_slack() {
		let yaml = r#"
id: slack-alerts
webhook: https://hooks.example.com/services/T00/B00/xxx
"#;
		let target: ExternalTarget = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(target.id, "slack-alerts");
		assert!(matches!(target.conn, TargetConnection::Slack(_)));
		if let TargetConnection::Slack(slack) = &target.conn {
			assert_eq!(
				slack.webhook.as_str(),
				"https://hooks.example.com/services/T00/B00/xxx"
			);
		}
	}

	#[test]
	fn test_external_target_slack_with_fields() {
		let yaml = r#"
id: slack-custom
webhook: https://hooks.example.com/services/T00/B00/xxx
fields:
  - name: text
    field: body
  - name: server
    value: production
"#;
		let target: ExternalTarget = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(target.id, "slack-custom");
		assert!(matches!(target.conn, TargetConnection::Slack(_)));
		if let TargetConnection::Slack(slack) = &target.conn {
			assert_eq!(slack.fields.len(), 2);
		}
	}

	#[test]
	fn test_alert_targets_mixed() {
		let yaml = r#"
targets:
  - id: email-team
    addresses:
      - team@example.com
  - id: slack-channel
    webhook: https://hooks.example.com/services/T00/B00/xxx
"#;
		let targets: AlertTargets = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(targets.targets.len(), 2);
		assert!(matches!(
			targets.targets[0].conn,
			TargetConnection::Email(_)
		));
		assert!(matches!(
			targets.targets[1].conn,
			TargetConnection::Slack(_)
		));
	}

	#[test]
	fn test_resolve_email_target() {
		let mut external_targets = HashMap::new();
		external_targets.insert(
			"ops".to_string(),
			vec![ExternalTarget {
				id: "ops".to_string(),
				conn: TargetConnection::Email(TargetEmail {
					addresses: vec!["ops@example.com".to_string()],
				}),
			}],
		);

		let send = SendTarget::Simple {
			id: "ops".to_string(),
			subject: Some("Test".to_string()),
			template: "Body".to_string(),
		};

		let resolved = send.resolve_external(&external_targets);
		assert_eq!(resolved.len(), 1);
		assert!(matches!(resolved[0].conn, TargetConnection::Email(_)));
	}

	#[test]
	fn test_resolve_slack_target() {
		let mut external_targets = HashMap::new();
		external_targets.insert(
			"slack".to_string(),
			vec![ExternalTarget {
				id: "slack".to_string(),
				conn: TargetConnection::Slack(TargetSlack {
					webhook: "https://hooks.example.com/services/T00/B00/xxx"
						.parse()
						.unwrap(),
					fields: slack::SlackField::default_set(),
				}),
			}],
		);

		let send = SendTarget::Simple {
			id: "slack".to_string(),
			subject: Some("Test".to_string()),
			template: "Body".to_string(),
		};

		let resolved = send.resolve_external(&external_targets);
		assert_eq!(resolved.len(), 1);
		assert!(matches!(resolved[0].conn, TargetConnection::Slack(_)));
	}

	#[test]
	fn test_resolve_mixed_targets_same_id() {
		let mut external_targets = HashMap::new();
		external_targets.insert(
			"all".to_string(),
			vec![
				ExternalTarget {
					id: "all".to_string(),
					conn: TargetConnection::Email(TargetEmail {
						addresses: vec!["team@example.com".to_string()],
					}),
				},
				ExternalTarget {
					id: "all".to_string(),
					conn: TargetConnection::Slack(TargetSlack {
						webhook: "https://hooks.example.com/services/T00/B00/xxx"
							.parse()
							.unwrap(),
						fields: slack::SlackField::default_set(),
					}),
				},
			],
		);

		let send = SendTarget::Simple {
			id: "all".to_string(),
			subject: Some("Test".to_string()),
			template: "Body".to_string(),
		};

		let resolved = send.resolve_external(&external_targets);
		assert_eq!(resolved.len(), 2);
		assert!(matches!(resolved[0].conn, TargetConnection::Email(_)));
		assert!(matches!(resolved[1].conn, TargetConnection::Slack(_)));
	}
}
