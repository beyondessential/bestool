use std::collections::HashMap;

use miette::Result;
use tera::Context as TeraCtx;
use tracing::{debug, error, info, warn};

use crate::{
	LogError,
	alert::{AlertDefinition, InternalContext},
	targets::{ExternalTarget, ResolvedTarget, determine_default_target},
};

/// Internal event types that can trigger alerts
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventType {
	SourceError,
	DefinitionError,
	Http,
}

impl EventType {
	pub fn as_str(&self) -> &'static str {
		match self {
			Self::SourceError => "source-error",
			Self::DefinitionError => "definition-error",
			Self::Http => "http",
		}
	}
}

/// Context data for an event
#[derive(Debug, Clone)]
pub enum EventContext {
	SourceError {
		alert_file: String,
		error_message: String,
	},
	DefinitionError {
		alert_file: String,
		error_message: String,
	},
	Http {
		message: String,
		subject: Option<String>,
		custom: serde_json::Value,
	},
}

impl EventContext {
	pub fn to_tera_context(&self) -> TeraCtx {
		let mut ctx = TeraCtx::new();
		match self {
			Self::SourceError {
				alert_file,
				error_message,
			} => {
				ctx.insert("alert_file", alert_file);
				ctx.insert("error_message", error_message);
			}
			Self::DefinitionError {
				alert_file,
				error_message,
			} => {
				ctx.insert("alert_file", alert_file);
				ctx.insert("error_message", error_message);
			}
			Self::Http {
				message,
				subject,
				custom,
			} => {
				ctx.insert("message", message);
				ctx.insert("subject", subject.as_deref().unwrap_or("Custom alert"));
				if let serde_json::Value::Object(map) = custom {
					for (key, value) in map {
						ctx.insert(key, value);
					}
				}
			}
		}
		ctx
	}
}

/// Manages event-triggered alerts
#[derive(Clone)]
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

		debug!(total_alerts = alerts.len(), "initializing event manager");

		// Separate event-based alerts from regular alerts
		for (alert, targets) in alerts {
			if let crate::alert::TicketSource::Event { event } = &alert.source {
				debug!(
					file = ?alert.file,
					event = event.as_str(),
					targets = targets.len(),
					"registered event alert"
				);
				event_alerts
					.entry(event.clone())
					.or_default()
					.push((alert, targets));
			}
		}

		info!(
			event_types = ?event_alerts.keys().collect::<Vec<_>>(),
			total_event_alerts = event_alerts.values().map(|v| v.len()).sum::<usize>(),
			"event manager initialized"
		);

		let default_target = determine_default_target(external_targets).map(|t| ResolvedTarget {
			subject: None,
			template: String::new(),
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
		info!(
			event = event_type.as_str(),
			has_alerts = self.event_alerts.contains_key(&event_type),
			has_default_target = self.default_target.is_some(),
			"triggering event"
		);

		// Check if there are explicit alerts for this event
		if let Some(alerts) = self.event_alerts.get(&event_type) {
			info!(count = alerts.len(), "executing event alerts");
			for (alert, targets) in alerts {
				let mut tera_ctx = crate::templates::build_context(alert, chrono::Utc::now());
				// Merge event context
				tera_ctx.extend(event_context.to_tera_context());

				for target in targets {
					if let Err(err) = target.send(alert, &mut tera_ctx, email, dry_run).await {
						error!(file = ?alert.file, "failed to send event alert: {}", LogError(&err));
					}
				}
			}
		} else if let Some(ref default_target) = self.default_target {
			// No explicit alert, use default target with event-specific template
			info!(
				event = event_type.as_str(),
				"using default target for event (no explicit alert configured)"
			);

			let (subject_template, body_template) = match event_type {
				EventType::SourceError => (
					"[bestool-alertd] {{ hostname }}: Failed alert: {{ alert_file }}".to_string(),
					"<pre>{{ error_message }}</pre>".to_string(),
				),
				EventType::DefinitionError => (
					"[bestool-alertd] {{ hostname }}: Invalid alert definition: {{ alert_file }}"
						.to_string(),
					"<pre>{{ error_message }}</pre>".to_string(),
				),
				EventType::Http => (
					"[bestool-alertd] {{ hostname }}: {{ subject }}".to_string(),
					"{{ message }}".to_string(),
				),
			};

			let default_target_for_event = ResolvedTarget {
				subject: Some(subject_template),
				template: body_template,
				conn: default_target.conn.clone(),
			};

			// Create a synthetic alert for the default notification
			let synthetic_alert = AlertDefinition {
				file: format!("[internal:{}]", event_type.as_str()).into(),
				enabled: true,
				interval: "0 seconds".to_string(),
				interval_duration: std::time::Duration::from_secs(0),
				always_send: crate::alert::AlwaysSend::Boolean(false),
				when_changed: crate::alert::WhenChanged::default(),
				send: Vec::new(),
				source: crate::alert::TicketSource::Event {
					event: event_type.clone(),
				},
			};

			let mut tera_ctx =
				crate::templates::build_context(&synthetic_alert, chrono::Utc::now());
			tera_ctx.extend(event_context.to_tera_context());

			if let Err(err) = default_target_for_event
				.send(&synthetic_alert, &mut tera_ctx, email, dry_run)
				.await
			{
				error!("failed to send default event alert: {}", LogError(&err));
			}
		} else {
			warn!(
				event = event_type.as_str(),
				"no alerts or default target for event, skipping notification"
			);
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
		assert_eq!(EventType::DefinitionError.as_str(), "definition-error");
		assert_eq!(EventType::Http.as_str(), "http");
	}

	#[test]
	fn test_event_type_serialization() {
		let event = EventType::SourceError;
		let yaml = serde_yaml::to_string(&event).unwrap();
		assert!(yaml.contains("source-error"));
	}

	#[test]
	fn test_event_context_to_tera_source_error() {
		let ctx = EventContext::SourceError {
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

	#[test]
	fn test_event_context_to_tera_http() {
		let ctx = EventContext::Http {
			message: "Test message".to_string(),
			subject: Some("Test subject".to_string()),
			custom: serde_json::json!({"extra": "data"}),
		};

		let tera_ctx = ctx.to_tera_context();
		assert_eq!(
			tera_ctx.get("message").unwrap().as_str().unwrap(),
			"Test message"
		);
		assert_eq!(
			tera_ctx.get("subject").unwrap().as_str().unwrap(),
			"Test subject"
		);
		assert_eq!(tera_ctx.get("extra").unwrap().as_str().unwrap(), "data");
	}

	#[test]
	fn test_event_context_http_default_subject() {
		let ctx = EventContext::Http {
			message: "Test message".to_string(),
			subject: None,
			custom: serde_json::json!({}),
		};

		let tera_ctx = ctx.to_tera_context();
		assert_eq!(
			tera_ctx.get("subject").unwrap().as_str().unwrap(),
			"Custom alert"
		);
	}

	#[test]
	fn test_event_type_definition_error() {
		let yaml = "definition-error";
		let event: EventType = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(event, EventType::DefinitionError);
	}

	#[test]
	fn test_event_context_to_tera_definition_error() {
		let ctx = EventContext::DefinitionError {
			alert_file: "/etc/alerts/broken.yml".to_string(),
			error_message: "Invalid YAML syntax".to_string(),
		};

		let tera_ctx = ctx.to_tera_context();
		assert_eq!(
			tera_ctx.get("alert_file").unwrap().as_str().unwrap(),
			"/etc/alerts/broken.yml"
		);
		assert_eq!(
			tera_ctx.get("error_message").unwrap().as_str().unwrap(),
			"Invalid YAML syntax"
		);
	}
}
