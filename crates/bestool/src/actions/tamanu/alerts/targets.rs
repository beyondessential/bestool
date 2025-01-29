use std::collections::HashMap;

use reqwest::Url;

use super::templates::TemplateField;

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case", tag = "target")]
pub enum SendTarget {
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
	Slack {
		subject: Option<String>,
		template: String,
		#[serde(flatten)]
		conn: TargetSlack,
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
	Zendesk {
		id: String,
		#[serde(flatten)]
		conn: TargetZendesk,
	},
	Slack {
		id: String,
		#[serde(flatten)]
		conn: TargetSlack,
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

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct TargetEmail {
	pub addresses: Vec<String>,
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct TargetZendesk {
	pub endpoint: Url,

	#[serde(flatten)]
	pub method: ZendeskMethod,

	pub ticket_form_id: Option<u64>,

	#[serde(default)]
	pub custom_fields: Vec<ZendeskCustomField>,
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

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(untagged, deny_unknown_fields)]
pub enum ZendeskMethod {
	// Make credentials and requester fields exclusive as specifying the requester object in authorized
	// request is invalid. We may be able to specify some account as the requester, but it's not
	// necessary. That's because the requester defaults to the authenticated account.
	Authorized { credentials: ZendeskCredentials },
	Anonymous { requester: String },
}

#[derive(serde::Deserialize, Clone, Debug)]
pub struct ZendeskCredentials {
	pub email: String,
	pub password: String,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct ZendeskCustomField {
	pub id: u64,
	pub value: String,
}
