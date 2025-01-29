use mailgun_rs::{EmailAddress, Mailgun, Message};
use miette::{IntoDiagnostic, Result, WrapErr};
use tracing::debug;

use crate::actions::tamanu::{alerts::definition::AlertDefinition, config::TamanuConfig};

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct TargetEmail {
	pub addresses: Vec<String>,
}

impl TargetEmail {
	pub async fn send(
		&self,
		alert: &AlertDefinition,
		config: &TamanuConfig,
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
		let sender = EmailAddress::address(&config.mailgun.sender);
		let mailgun = Mailgun {
			api_key: config.mailgun.api_key.clone(),
			domain: config.mailgun.domain.clone(),
		};
		let message = Message {
			to: self
				.addresses
				.iter()
				.map(|email| EmailAddress::address(email))
				.collect(),
			subject: subject.into(),
			html: body.into(),
			..Default::default()
		};
		mailgun
			.async_send(mailgun_rs::MailgunRegion::US, &sender, message)
			.await
			.into_diagnostic()
			.wrap_err("sending email")
			.map(drop)
	}
}
