use std::{
	collections::HashMap, io::Write, ops::ControlFlow, path::PathBuf, process::Stdio, sync::Arc,
	time::Duration,
};

use chrono::{DateTime, Utc};
use miette::{Context as _, IntoDiagnostic, Result, miette};
use tera::Context as TeraCtx;
use tokio::io::AsyncReadExt as _;
use tokio_postgres::types::ToSql;
use tracing::{debug, error, info, instrument, warn};

use crate::{
	EmailConfig, LogError, events::EventType, targets::ExternalTarget, templates::build_context,
};

fn enabled() -> bool {
	true
}

fn default_interval() -> String {
	"1 minute".to_string()
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct NumericalThreshold {
	pub field: String,
	pub alert_at: f64,
	pub clear_at: Option<f64>,
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(untagged)]
pub enum WhenChanged {
	Boolean(bool),
	Detailed(WhenChangedConfig),
}

impl Default for WhenChanged {
	fn default() -> Self {
		WhenChanged::Boolean(false)
	}
}

#[derive(serde::Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct WhenChangedConfig {
	#[serde(default)]
	pub except: Vec<String>,
	#[serde(default)]
	pub only: Vec<String>,
}

#[derive(serde::Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct AlertDefinition {
	#[serde(default, skip)]
	pub file: PathBuf,

	#[serde(default = "enabled")]
	pub enabled: bool,

	#[serde(default = "default_interval")]
	pub interval: String,

	#[serde(skip)]
	pub interval_duration: Duration,

	#[serde(default)]
	pub always_send: bool,

	#[serde(default)]
	pub when_changed: WhenChanged,

	#[serde(default)]
	pub send: Vec<crate::targets::SendTarget>,

	#[serde(flatten)]
	pub source: TicketSource,
}

#[derive(serde::Deserialize, Debug, Default, Clone)]
#[serde(untagged, deny_unknown_fields)]
pub enum TicketSource {
	Sql {
		sql: String,
		#[serde(default)]
		numerical: Vec<NumericalThreshold>,
	},
	Shell {
		shell: String,
		run: String,
	},
	Event {
		event: EventType,
	},

	#[default]
	None,
}

impl AlertDefinition {
	pub fn normalise(
		mut self,
		external_targets: &HashMap<String, Vec<ExternalTarget>>,
	) -> Result<(Self, Vec<crate::targets::ResolvedTarget>)> {
		// Parse interval string into duration
		self.interval_duration = parse_interval(&self.interval)
			.wrap_err_with(|| format!("failed to parse interval: {}", self.interval))?;

		// Validate templates before resolving targets
		// This catches template syntax errors early
		for (idx, target) in self.send.iter().enumerate() {
			crate::templates::load_templates(target.subject(), target.template()).wrap_err_with(
				|| {
					format!(
						"validating templates for send target #{} (id: {})",
						idx + 1,
						target.id()
					)
				},
			)?;
		}

		let resolved = self
			.send
			.iter()
			.flat_map(|target| {
				let resolved_targets = target.resolve_external(external_targets);
				if resolved_targets.is_empty() {
					error!(
						file=?self.file,
						id = %target.id(),
						available_targets=?external_targets.keys().collect::<Vec<_>>(),
						"external target not found"
					);
				}
				resolved_targets
			})
			.collect();

		self.send.clear(); // Clear send targets after resolution
		Ok((self, resolved))
	}

	#[instrument(skip(self, pool, not_before, context))]
	pub async fn read_sources(
		&self,
		pool: &bestool_postgres::pool::PgPool,
		not_before: DateTime<Utc>,
		context: &mut TeraCtx,
		was_triggered: bool,
	) -> Result<ControlFlow<(), ()>> {
		match &self.source {
			TicketSource::None => {
				debug!(?self.file, "no source, skipping");
				return Ok(ControlFlow::Break(()));
			}
			TicketSource::Event { .. } => {
				// Event sources are triggered externally, not by this method
				debug!(?self.file, "event source, skipping normal execution");
				return Ok(ControlFlow::Break(()));
			}
			TicketSource::Sql { sql, numerical } => {
				let client = pool
					.get()
					.await
					.map_err(|e| miette!("getting connection from pool: {e}"))?;
				let statement = client.prepare(sql).await.into_diagnostic()?;

				let interval = bestool_postgres::pg_interval::Interval(self.interval_duration);
				let all_params: Vec<&(dyn ToSql + Sync)> = vec![&not_before, &interval];

				let rows = client
					.query(&statement, &all_params[..statement.params().len()])
					.await
					.into_diagnostic()
					.wrap_err("querying database")?;

				if rows.is_empty() {
					debug!(?self.file, "no rows returned, skipping");
					return Ok(ControlFlow::Break(()));
				}

				let context_rows = rows_to_value_map(&rows);

				// Check numerical thresholds if configured
				if !numerical.is_empty() {
					let triggered =
						check_numerical_thresholds(&context_rows, numerical, was_triggered)?;
					if !triggered {
						debug!(?self.file, "numerical thresholds not met, skipping");
						return Ok(ControlFlow::Break(()));
					}
				}

				info!(?self.file, rows=%rows.len(), "alert triggered");
				context.insert("rows", &context_rows);
			}
			TicketSource::Shell { shell, run } => {
				let mut script = tempfile::Builder::new().tempfile().into_diagnostic()?;
				write!(script.as_file_mut(), "{run}").into_diagnostic()?;

				let mut shell = tokio::process::Command::new(shell)
					.arg(script.path())
					.stdin(Stdio::null())
					.stdout(Stdio::piped())
					.spawn()
					.into_diagnostic()?;

				let mut output = Vec::new();
				let mut stdout = shell
					.stdout
					.take()
					.ok_or_else(|| miette!("getting the child stdout handle"))?;
				let output_future =
					futures::future::try_join(shell.wait(), stdout.read_to_end(&mut output));

				let Ok(res) = tokio::time::timeout(self.interval_duration, output_future).await
				else {
					warn!(?self.file, "the script timed out, skipping");
					shell.kill().await.into_diagnostic()?;
					return Ok(ControlFlow::Break(()));
				};

				let (status, output_size) = res.into_diagnostic().wrap_err("running the shell")?;

				if status.success() {
					debug!(?self.file, "the script succeeded, skipping");
					return Ok(ControlFlow::Break(()));
				}
				info!(?self.file, ?status, ?output_size, "alert triggered");

				context.insert("output", &String::from_utf8_lossy(&output));
			}
		}
		Ok(ControlFlow::Continue(()))
	}

	pub async fn execute(
		&self,
		ctx: Arc<InternalContext>,
		email: Option<&EmailConfig>,
		dry_run: bool,
		resolved_targets: &[crate::targets::ResolvedTarget],
	) -> Result<()> {
		info!(?self.file, "executing alert");

		let now = chrono::Utc::now();
		let not_before = now - self.interval_duration;
		info!(?now, ?not_before, interval=?self.interval_duration, "date range for alert");

		let mut tera_ctx = build_context(self, now);
		if self
			.read_sources(&ctx.pg_pool, not_before, &mut tera_ctx, false)
			.await?
			.is_break()
		{
			return Ok(());
		}

		for target in resolved_targets {
			if let Err(err) = target.send(self, &mut tera_ctx, email, dry_run).await {
				error!("sending: {}", LogError(&err));
			}
		}

		Ok(())
	}
}

#[derive(Debug, Clone)]
pub struct InternalContext {
	pub pg_pool: bestool_postgres::pool::PgPool,
}

fn rows_to_value_map(
	rows: &[tokio_postgres::Row],
) -> Vec<serde_json::Map<String, serde_json::Value>> {
	rows.iter()
		.map(|row| {
			let mut map = serde_json::Map::new();
			for (idx, column) in row.columns().iter().enumerate() {
				let value = bestool_postgres::stringify::postgres_to_json_value(row, idx);
				map.insert(column.name().to_string(), value);
			}
			map
		})
		.collect()
}

fn check_numerical_thresholds(
	rows: &[serde_json::Map<String, serde_json::Value>],
	thresholds: &[NumericalThreshold],
	was_triggered: bool,
) -> Result<bool> {
	for threshold in thresholds {
		for row in rows {
			let value = match row.get(&threshold.field) {
				Some(serde_json::Value::Number(n)) => n
					.as_f64()
					.ok_or_else(|| miette!("field '{}' is not a valid number", threshold.field))?,
				Some(_) => {
					return Err(miette!(
						"field '{}' exists but is not a number",
						threshold.field
					));
				}
				None => {
					return Err(miette!(
						"field '{}' not found in query results",
						threshold.field
					));
				}
			};

			// Determine if we're checking for "above" or "below" based on clear_at
			let is_inverted = threshold
				.clear_at
				.is_some_and(|clear| clear > threshold.alert_at);

			if was_triggered {
				// Already triggered, check if we should clear
				if let Some(clear_at) = threshold.clear_at {
					let should_clear = if is_inverted {
						// Inverted: clear when value >= clear_at
						value >= clear_at
					} else {
						// Normal: clear when value <= clear_at
						value <= clear_at
					};

					if should_clear {
						// This threshold has cleared, continue checking others
						continue;
					} else {
						// Still above/below clear threshold, remain triggered
						return Ok(true);
					}
				} else {
					// No clear_at specified, check alert_at threshold
					let still_triggered = if is_inverted {
						value <= threshold.alert_at
					} else {
						value >= threshold.alert_at
					};

					if still_triggered {
						return Ok(true);
					}
				}
			} else {
				// Not yet triggered, check if we should trigger
				let should_trigger = if is_inverted {
					// Inverted: trigger when value <= alert_at
					value <= threshold.alert_at
				} else {
					// Normal: trigger when value >= alert_at
					value >= threshold.alert_at
				};

				if should_trigger {
					return Ok(true);
				}
			}
		}
	}

	Ok(false)
}

fn parse_interval(s: &str) -> Result<Duration> {
	let s = s.trim();

	// Try to parse as a simple number (seconds)
	if let Ok(secs) = s.parse::<u64>() {
		return Ok(Duration::from_secs(secs));
	}

	// Parse with units
	let parts: Vec<&str> = s.split_whitespace().collect();
	if parts.len() != 2 {
		return Err(miette!(
			"interval must be in format '<number> <unit>' or just '<seconds>'"
		));
	}

	let value: u64 = parts[0]
		.parse()
		.into_diagnostic()
		.wrap_err("interval value must be a number")?;
	let unit = parts[1].to_lowercase();

	let duration = match unit.as_str() {
		"second" | "seconds" | "s" | "sec" | "secs" => Duration::from_secs(value),
		"minute" | "minutes" | "m" | "min" | "mins" => Duration::from_secs(value * 60),
		"hour" | "hours" | "h" | "hr" | "hrs" => Duration::from_secs(value * 3600),
		"day" | "days" | "d" => Duration::from_secs(value * 86400),
		_ => {
			return Err(miette!(
				"unknown interval unit: {}, expected: seconds, minutes, hours, or days",
				unit
			));
		}
	};

	Ok(duration)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_alert_with_event_source() {
		let yaml = r#"
event: source-error
send:
  - id: test-target
    subject: Test
    template: Test template
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		assert!(matches!(alert.source, TicketSource::Event { .. }));
		if let TicketSource::Event { event } = alert.source {
			assert_eq!(event, EventType::SourceError);
		}
	}

	#[test]
	fn test_parse_interval() {
		assert_eq!(parse_interval("60").unwrap(), Duration::from_secs(60));
		assert_eq!(parse_interval("1 minute").unwrap(), Duration::from_secs(60));
		assert_eq!(
			parse_interval("5 minutes").unwrap(),
			Duration::from_secs(300)
		);
		assert_eq!(
			parse_interval("2 hours").unwrap(),
			Duration::from_secs(7200)
		);
		assert_eq!(parse_interval("1 day").unwrap(), Duration::from_secs(86400));
		assert_eq!(
			parse_interval("30 seconds").unwrap(),
			Duration::from_secs(30)
		);
	}

	#[test]
	fn test_default_interval() {
		let yaml = r#"
sql: "SELECT 1"
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		assert_eq!(alert.interval, "1 minute");
	}

	#[test]
	fn test_default_always_send() {
		let yaml = r#"
sql: "SELECT 1"
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		assert!(!alert.always_send);
	}

	#[test]
	fn test_always_send_true() {
		let yaml = r#"
always-send: true
sql: "SELECT 1"
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		assert!(alert.always_send);
	}

	#[test]
	fn test_numerical_threshold_normal() {
		let yaml = r#"
sql: "SELECT 1"
numerical:
  - field: count
    alert-at: 100
    clear-at: 50
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		if let TicketSource::Sql { numerical, .. } = &alert.source {
			assert_eq!(numerical.len(), 1);
			assert_eq!(numerical[0].field, "count");
			assert_eq!(numerical[0].alert_at, 100.0);
			assert_eq!(numerical[0].clear_at, Some(50.0));
		} else {
			panic!("Expected Sql source");
		}
	}

	#[test]
	fn test_numerical_threshold_inverted() {
		let yaml = r#"
sql: "SELECT 1"
numerical:
  - field: free_space
    alert-at: 10
    clear-at: 50
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		if let TicketSource::Sql { numerical, .. } = &alert.source {
			assert_eq!(numerical.len(), 1);
			assert_eq!(numerical[0].field, "free_space");
			assert_eq!(numerical[0].alert_at, 10.0);
			assert_eq!(numerical[0].clear_at, Some(50.0));
		} else {
			panic!("Expected Sql source");
		}
	}

	#[test]
	fn test_numerical_threshold_no_clear() {
		let yaml = r#"
sql: "SELECT 1"
numerical:
  - field: errors
    alert-at: 5
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		if let TicketSource::Sql { numerical, .. } = &alert.source {
			assert_eq!(numerical.len(), 1);
			assert_eq!(numerical[0].field, "errors");
			assert_eq!(numerical[0].alert_at, 5.0);
			assert_eq!(numerical[0].clear_at, None);
		} else {
			panic!("Expected Sql source");
		}
	}

	#[test]
	fn test_check_numerical_thresholds_normal_trigger() {
		let mut row = serde_json::Map::new();
		row.insert("count".to_string(), serde_json::Value::Number(150.into()));
		let rows = vec![row];

		let threshold = NumericalThreshold {
			field: "count".to_string(),
			alert_at: 100.0,
			clear_at: Some(50.0),
		};

		// Not triggered yet, value 150 >= alert_at 100, should trigger
		let result =
			check_numerical_thresholds(&rows, std::slice::from_ref(&threshold), false).unwrap();
		assert!(result);

		// Already triggered, value 150 > clear_at 50, should stay triggered
		let result =
			check_numerical_thresholds(&rows, std::slice::from_ref(&threshold), true).unwrap();
		assert!(result);

		// Already triggered, value 30 <= clear_at 50, should clear
		let mut row = serde_json::Map::new();
		row.insert("count".to_string(), serde_json::Value::Number(30.into()));
		let rows = vec![row];
		let result = check_numerical_thresholds(&rows, &[threshold], true).unwrap();
		assert!(!result);
	}

	#[test]
	fn test_check_numerical_thresholds_inverted_trigger() {
		let mut row = serde_json::Map::new();
		row.insert(
			"free_space".to_string(),
			serde_json::Value::Number(5.into()),
		);
		let rows = vec![row];

		let threshold = NumericalThreshold {
			field: "free_space".to_string(),
			alert_at: 10.0,
			clear_at: Some(50.0), // Inverted because clear_at > alert_at
		};

		// Not triggered yet, value 5 <= alert_at 10, should trigger (inverted)
		let result =
			check_numerical_thresholds(&rows, std::slice::from_ref(&threshold), false).unwrap();
		assert!(result);

		// Already triggered, value 5 < clear_at 50, should stay triggered
		let result =
			check_numerical_thresholds(&rows, std::slice::from_ref(&threshold), true).unwrap();
		assert!(result);

		// Already triggered, value 60 >= clear_at 50, should clear
		let mut row = serde_json::Map::new();
		row.insert(
			"free_space".to_string(),
			serde_json::Value::Number(60.into()),
		);
		let rows = vec![row];
		let result = check_numerical_thresholds(&rows, &[threshold], true).unwrap();
		assert!(!result);
	}

	#[test]
	fn test_check_numerical_thresholds_no_clear_at() {
		let threshold = NumericalThreshold {
			field: "errors".to_string(),
			alert_at: 5.0,
			clear_at: None,
		};

		// Trigger when >= 5
		let mut row = serde_json::Map::new();
		row.insert("errors".to_string(), serde_json::Value::Number(10.into()));
		let rows = vec![row];
		let result =
			check_numerical_thresholds(&rows, std::slice::from_ref(&threshold), false).unwrap();
		assert!(result);

		// Still triggered when >= 5
		let result =
			check_numerical_thresholds(&rows, std::slice::from_ref(&threshold), true).unwrap();
		assert!(result);

		// Clear when < 5
		let mut row = serde_json::Map::new();
		row.insert("errors".to_string(), serde_json::Value::Number(3.into()));
		let rows = vec![row];
		let result = check_numerical_thresholds(&rows, &[threshold], true).unwrap();
		assert!(!result);
	}

	#[test]
	fn test_when_changed_boolean_true() {
		let yaml = r#"
sql: "SELECT 1"
when-changed: true
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		assert!(matches!(alert.when_changed, WhenChanged::Boolean(true)));
	}

	#[test]
	fn test_when_changed_boolean_false() {
		let yaml = r#"
sql: "SELECT 1"
when-changed: false
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		assert!(matches!(alert.when_changed, WhenChanged::Boolean(false)));
	}

	#[test]
	fn test_when_changed_default() {
		let yaml = r#"
sql: "SELECT 1"
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		assert!(matches!(alert.when_changed, WhenChanged::Boolean(false)));
	}

	#[test]
	fn test_when_changed_except() {
		let yaml = r#"
sql: "SELECT 1"
when-changed:
  except: [created_at, updated_at]
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		match &alert.when_changed {
			WhenChanged::Detailed(config) => {
				assert_eq!(config.except, vec!["created_at", "updated_at"]);
				assert!(config.only.is_empty());
			}
			_ => panic!("Expected Detailed variant"),
		}
	}

	#[test]
	fn test_when_changed_only() {
		let yaml = r#"
sql: "SELECT 1"
when-changed:
  only: [error, message]
send:
  - id: test
    subject: Test
    template: Test
"#;
		let alert: AlertDefinition = serde_yaml::from_str(yaml).unwrap();
		match &alert.when_changed {
			WhenChanged::Detailed(config) => {
				assert_eq!(config.only, vec!["error", "message"]);
				assert!(config.except.is_empty());
			}
			_ => panic!("Expected Detailed variant"),
		}
	}
}
