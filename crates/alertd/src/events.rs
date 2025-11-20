use std::collections::HashMap;

use miette::Result;
use tera::Context as TeraCtx;
use tracing::{debug, error, info};

use crate::{
	alert::{AlertDefinition, InternalContext},
	targets::{ExternalTarget, ResolvedTarget, determine_default_target},
};

/// Internal event types that can trigger alerts
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventType {
	SourceError,
}

impl EventType {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::SourceError => "source-error",
		}
	}
}

/// Context data for an event
pub struct EventContext {
	pub alert_file: String,
	pub error_message: String,
}

impl EventContext {
	pub fn to_tera_context(&self) -> TeraCtx {
		let mut ctx = TeraCtx::new();
		ctx.insert("alert_file", &self.alert_file);
		ctx.insert("error_message", &self.error_message);
		ctx
	}
}

/// Manages event-triggered alerts
pub struct EventManager {
	/// Alerts that listen for specific events
	event_alerts: HashMap<EventType, Vec<(AlertDefinition, Vec<ResolvedTarget>)>>,
	/// Default target for fallback alerts
	default_target: Option<ResolvedTarget>,
}

impl EventManager {
	pub fn new(
		alerts: Vec<(AlertDefinition, Vec<ResolvedTarget>)>,
		external_targets: &HashMap<String, Vec<ExternalTarget>>,
	) -> Self {
		let mut event_alerts: HashMap<EventType, Vec<(AlertDefinition, Vec<ResolvedTarget>)>> =
			HashMap::new();

		// Separate event-based alerts from regular alerts
		for (alert, targets) in alerts {
			if let crate::alert::TicketSource::Event { event } = &alert.source {
				debug!(
					file = ?alert.file,
					event = event.as_str(),
					"registered event alert"
				);
				event_alerts
					.entry(event.clone())
					.or_default()
					.push((alert, targets));
			}
		}

		let default_target = determine_default_target(external_targets).map(|t| ResolvedTarget {
			subject: Some("[Tamanu Alert] Failed alert: {{ alert_file }}".to_string()),
			template: "{{ error_message }}".to_string(),
			conn: t.conn.clone(),
		});
		if let Some(ref target) = default_target {
			info!(
				from = target
					.conn
					.addresses
					.first()
					.map(|s| s.as_str())
					.unwrap_or("unknown"),
				"determined default target for fallback alerts"
			);
		}

		Self {
			event_alerts,
			default_target,
		}
	}

	/// Trigger an event with the given context
	pub async fn trigger_event(
		&self,
		event_type: EventType,
		_ctx: &InternalContext,
		email: Option<&crate::EmailConfig>,
		dry_run: bool,
		event_context: EventContext,
	) -> Result<()> {
		info!(event = event_type.as_str(), "triggering event");

		// Check if there are explicit alerts for this event
		if let Some(alerts) = self.event_alerts.get(&event_type) {
			debug!(count = alerts.len(), "executing event alerts");
			for (alert, targets) in alerts {
				let mut tera_ctx = crate::templates::build_context(alert, chrono::Utc::now());
				// Merge event context
				tera_ctx.extend(event_context.to_tera_context());

				for target in targets {
					if let Err(err) = target.send(alert, &mut tera_ctx, email, dry_run).await {
						error!(file = ?alert.file, "failed to send event alert: {err:?}");
					}
				}
			}
		} else if let Some(ref default_target) = self.default_target {
			// No explicit alert, use default target
			debug!("using default target for event");

			// Create a synthetic alert for the default notification
			let synthetic_alert = AlertDefinition {
				file: format!("[internal:{}]", event_type.as_str()).into(),
				enabled: true,
				interval: std::time::Duration::from_secs(0),
				send: Vec::new(),
				source: crate::alert::TicketSource::Event {
					event: event_type.clone(),
				},
			};

			let mut tera_ctx =
				crate::templates::build_context(&synthetic_alert, chrono::Utc::now());
			tera_ctx.extend(event_context.to_tera_context());

			if let Err(err) = default_target
				.send(&synthetic_alert, &mut tera_ctx, email, dry_run)
				.await
			{
				error!("failed to send default event alert: {err:?}");
			}
		} else {
			debug!("no alerts or default target for event, skipping");
		}

		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_event_type_parsing() {
		let yaml = "source-error";
		let event: EventType = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(event, EventType::SourceError);
	}

	#[test]
	fn test_event_type_as_str() {
		assert_eq!(EventType::SourceError.as_str(), "source-error");
	}

	#[test]
	fn test_event_type_serialization() {
		let event = EventType::SourceError;
		let yaml = serde_yaml::to_string(&event).unwrap();
		assert!(yaml.contains("source-error"));
	}

	#[test]
	fn test_event_context_to_tera() {
		let ctx = EventContext {
			alert_file: "/etc/alerts/test.yml".to_string(),
			error_message: "Something went wrong".to_string(),
		};

		let tera_ctx = ctx.to_tera_context();
		assert_eq!(
			tera_ctx.get("alert_file").unwrap().as_str().unwrap(),
			"/etc/alerts/test.yml"
		);
		assert_eq!(
			tera_ctx.get("error_message").unwrap().as_str().unwrap(),
			"Something went wrong"
		);
	}
}
