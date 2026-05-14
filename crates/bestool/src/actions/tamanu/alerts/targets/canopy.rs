use bestool_alertd::canopy::{DEFAULT_CANOPY_URL, NewEvent, Severity};
use jiff::Timestamp;
use miette::{Result, miette};
use reqwest::Url;
use sysinfo::System;
use tracing::debug;

use crate::actions::tamanu::alerts::{InternalContext, definition::AlertDefinition};

fn default_canopy_url() -> Url {
	DEFAULT_CANOPY_URL.parse().expect("default canopy URL is valid")
}

#[derive(serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub struct TargetCanopy {
	#[serde(default = "default_canopy_url")]
	pub url: Url,
	pub source: String,
	#[serde(default)]
	pub severity: Option<Severity>,
}

/// Build the deduplication ref for a canopy event.
///
/// Combines hostname, alert file stem, and target id so the same alert firing
/// on different hosts or to different canopy targets produces distinct issues.
fn build_ref(alert: &AlertDefinition, target_id: &str) -> String {
	let hostname = System::host_name().unwrap_or_else(|| "unknown".into());
	let stem = alert
		.file
		.file_stem()
		.map(|s| s.to_string_lossy().into_owned())
		.unwrap_or_else(|| "alert".into());
	format!("{hostname}/{stem}:{target_id}")
}

impl TargetCanopy {
	pub async fn send(
		&self,
		ctx: &InternalContext,
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
			println!("Recipients: canopy:{}", self.url);
			println!("Source: {}", self.source);
			println!("Ref: {ref}", ref = r#ref);
			println!("Severity: {:?}", self.severity.unwrap_or(Severity::Error));
			println!("Active: true");
			println!("Subject: {subject}");
			println!("Body: {body}");
			return Ok(());
		}

		let client = ctx
			.canopy_client
			.as_deref()
			.ok_or_else(|| miette!("canopy target {target_id} configured but no device key was loaded"))?;

		debug!(?alert.file, target_id, "sending canopy trigger event");

		client
			.post_event(
				&self.url,
				NewEvent {
					source: &self.source,
					r#ref: &r#ref,
					message: body,
					description: Some(subject),
					severity: Some(self.severity.unwrap_or(Severity::Error)),
					occurred_at: Some(Timestamp::now()),
					active: Some(true),
				},
			)
			.await
	}

	pub async fn send_clear(
		&self,
		ctx: &InternalContext,
		alert: &AlertDefinition,
		target_id: &str,
		dry_run: bool,
	) -> Result<()> {
		let r#ref = build_ref(alert, target_id);

		if dry_run {
			println!("-------------------------------");
			println!("Alert (cleared): {}", alert.file.display());
			println!("Recipients: canopy:{}", self.url);
			println!("Source: {}", self.source);
			println!("Ref: {ref}", ref = r#ref);
			println!("Active: false");
			return Ok(());
		}

		let Some(client) = ctx.canopy_client.as_deref() else {
			debug!(target_id, "no device key loaded, skipping canopy clear");
			return Ok(());
		};

		debug!(?alert.file, target_id, "sending canopy clear event");

		client
			.post_event(
				&self.url,
				NewEvent {
					source: &self.source,
					r#ref: &r#ref,
					message: "alert cleared",
					description: None,
					severity: Some(self.severity.unwrap_or(Severity::Error)),
					occurred_at: Some(Timestamp::now()),
					active: Some(false),
				},
			)
			.await
	}
}
