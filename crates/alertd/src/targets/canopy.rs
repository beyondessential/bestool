use jiff::Timestamp;
use miette::{Result, miette};
use sysinfo::System;
use tracing::debug;
use url::Url;

use crate::{
	alert::AlertDefinition,
	canopy::{CanopyClient, DEFAULT_CANOPY_URL, NewEvent, Severity},
};

fn default_canopy_url() -> Url {
	DEFAULT_CANOPY_URL
		.parse()
		.expect("default canopy URL is valid")
}

/// External-target connection for a canopy events endpoint.
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct TargetCanopy {
	pub canopy: CanopyConfig,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct CanopyConfig {
	#[serde(default = "default_canopy_url")]
	pub url: Url,
	pub source: String,
	#[serde(default)]
	pub severity: Option<Severity>,
}

/// Build the deduplication ref for a canopy event.
///
/// Combines the hostname, alert file stem, and target id so the same alert
/// firing on different hosts or to different canopy targets produces
/// distinct canopy issues.
pub fn build_ref(alert: &AlertDefinition, target_id: &str) -> String {
	let hostname = System::host_name().unwrap_or_else(|| "unknown".into());
	let stem = alert
		.file
		.file_stem()
		.map(|s| s.to_string_lossy().into_owned())
		.unwrap_or_else(|| "alert".into());
	format!("{hostname}/{stem}:{target_id}")
}

impl TargetCanopy {
	/// Post a triggering event to canopy.
	pub async fn send(
		&self,
		client: Option<&CanopyClient>,
		alert: &AlertDefinition,
		target_id: &str,
		subject: &str,
		body: &str,
		dry_run: bool,
	) -> Result<()> {
		let r#ref = build_ref(alert, target_id);

		if dry_run {
			println!("-------------------------------");
			println!("Alert: {}", alert.file.display());
			println!("Recipients: canopy:{}", self.canopy.url);
			println!("Source: {}", self.canopy.source);
			println!("Ref: {ref}", ref = r#ref);
			println!(
				"Severity: {:?}",
				self.canopy.severity.unwrap_or(Severity::Error)
			);
			println!("Active: true");
			println!("Subject: {subject}");
			println!("Body: {body}");
			return Ok(());
		}

		let client = client.ok_or_else(|| {
			miette!(
				"canopy target {target_id} configured but no device key was provided to the daemon"
			)
		})?;

		debug!(?alert.file, target_id, "sending canopy trigger event");

		client
			.post_event(
				&self.canopy.url,
				NewEvent {
					source: &self.canopy.source,
					r#ref: &r#ref,
					message: subject,
					description: Some(body),
					severity: Some(self.canopy.severity.unwrap_or(Severity::Error)),
					occurred_at: Some(Timestamp::now()),
					active: Some(true),
				},
			)
			.await
	}

	/// Post a clearing event to canopy.
	pub async fn send_clear(
		&self,
		client: Option<&CanopyClient>,
		alert: &AlertDefinition,
		target_id: &str,
		dry_run: bool,
	) -> Result<()> {
		let r#ref = build_ref(alert, target_id);

		if dry_run {
			println!("-------------------------------");
			println!("Alert (cleared): {}", alert.file.display());
			println!("Recipients: canopy:{}", self.canopy.url);
			println!("Source: {}", self.canopy.source);
			println!("Ref: {ref}", ref = r#ref);
			println!("Active: false");
			return Ok(());
		}

		let client = client.ok_or_else(|| {
			miette!(
				"canopy target {target_id} configured but no device key was provided to the daemon"
			)
		})?;

		debug!(?alert.file, target_id, "sending canopy clear event");

		client
			.post_event(
				&self.canopy.url,
				NewEvent {
					source: &self.canopy.source,
					r#ref: &r#ref,
					message: "alert cleared",
					description: None,
					severity: Some(self.canopy.severity.unwrap_or(Severity::Error)),
					occurred_at: Some(Timestamp::now()),
					active: Some(false),
				},
			)
			.await
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::targets::{ExternalTarget, TargetConnection};

	#[test]
	fn parse_canopy_external_target() {
		let yaml = r#"
id: meta
canopy:
  url: https://meta.tamanu.app
  source: my-server
  severity: warning
"#;
		let target: ExternalTarget = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(target.id, "meta");
		match target.conn {
			TargetConnection::Canopy(canopy) => {
				assert_eq!(canopy.canopy.url.as_str(), "https://meta.tamanu.app/");
				assert_eq!(canopy.canopy.source, "my-server");
				assert_eq!(canopy.canopy.severity, Some(Severity::Warning));
			}
			_ => panic!("expected canopy target"),
		}
	}

	#[test]
	fn parse_canopy_with_default_url() {
		let yaml = r#"
id: meta
canopy:
  source: my-server
"#;
		let target: ExternalTarget = serde_yaml::from_str(yaml).unwrap();
		match target.conn {
			TargetConnection::Canopy(canopy) => {
				assert_eq!(canopy.canopy.url.as_str(), "https://meta.tamanu.app/");
				assert_eq!(canopy.canopy.severity, None);
			}
			_ => panic!("expected canopy target"),
		}
	}
}
