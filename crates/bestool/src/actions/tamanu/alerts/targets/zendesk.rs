use miette::{IntoDiagnostic, Result, WrapErr};
use reqwest::Url;
use serde_json::json;

use crate::actions::tamanu::alerts::{definition::AlertDefinition, InternalContext};

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

impl TargetZendesk {
	pub async fn send(
		&self,
		alert: &AlertDefinition,
		ctx: &InternalContext,
		subject: &str,
		body: &str,
		requester: Option<&str>,
		dry_run: bool,
	) -> Result<()> {
		if dry_run {
			println!("-------------------------------");
			println!("Alert: {}", alert.file.display());
			println!("Endpoint: {}", self.endpoint);
			println!("Subject: {subject}");
			println!("Body: {body}");
			return Ok(());
		}

		let req = json!({
			"request": {
				"subject": subject,
				"ticket_form_id": self.ticket_form_id,
				"custom_fields": self.custom_fields,
				"comment": { "html_body": body },
				"requester": requester.map(|r| json!({ "name": r }))
			}
		});

		let mut req_builder = ctx.http_client.post(self.endpoint.clone()).json(&req);

		if let ZendeskMethod::Authorized {
			credentials: ZendeskCredentials { email, password },
		} = &self.method
		{
			req_builder =
				req_builder.basic_auth(std::format_args!("{email}/token"), Some(password));
		}

		req_builder
			.send()
			.await
			.into_diagnostic()
			.wrap_err("creating Zendesk ticket")
			.map(drop)
	}
}
