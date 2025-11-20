use mailgun_rs::{EmailAddress, Mailgun, Message};
use miette::{IntoDiagnostic, Result, WrapErr, miette};
use tracing::debug;

use crate::{alert::AlertDefinition, config::Config};

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct TargetEmail {
	pub addresses: Vec<String>,
}

impl TargetEmail {
	pub async fn send(
		&self,
		alert: &AlertDefinition,
		config: &Config,
		subject: &str,
		body: &str,
		dry_run: bool,
	) -> Result<()> {
		let body = {
			let parser = pulldown_cmark::Parser::new(body);
			let mut html_output = String::new();
			pulldown_cmark::html::push_html(&mut html_output, parser);
			html_output
		};

		if dry_run {
			println!("-------------------------------");
			println!("Alert: {}", alert.file.display());
			println!("Recipients: {}", self.addresses.join(", "));
			println!("Subject: {subject}");
			println!("Body: {body}");
			return Ok(());
		}

		debug!(?self.addresses, "sending email");
		let email_config = config
			.email
			.as_ref()
			.ok_or_else(|| miette!("missing email config"))?;
		let sender = EmailAddress::address(&email_config.from);
		let mailgun = Mailgun {
			api_key: email_config.mailgun_api_key.clone(),
			domain: email_config.mailgun_domain.clone(),
			message: Message {
				to: self
					.addresses
					.iter()
					.map(|email| EmailAddress::address(email))
					.collect(),
				subject: subject.into(),
				html: body,
				..Default::default()
			},
		};
		mailgun
			.async_send(mailgun_rs::MailgunRegion::US, &sender)
			.await
			.into_diagnostic()
			.wrap_err("sending email")
			.map(drop)
	}
}
