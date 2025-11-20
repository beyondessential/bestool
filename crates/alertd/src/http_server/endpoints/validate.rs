use axum::{Json, http::StatusCode, response::IntoResponse};

use crate::{
	alert::{AlertDefinition, TicketSource},
	http_server::types::{ValidationInfo, ValidationResponse},
};

pub async fn handle_validate(body: String) -> impl IntoResponse {
	// Try to parse as YAML with serde_path_to_error for better error messages
	let deserializer = serde_yaml::Deserializer::from_str(&body);
	let alert: AlertDefinition = match serde_path_to_error::deserialize(deserializer) {
		Ok(alert) => alert,
		Err(err) => {
			// Parse error - return detailed error information
			let path = err.path().to_string();
			let inner = err.into_inner();
			let error_msg = format!("{}", inner);

			// The inner error is already a serde_yaml::Error, extract location if available
			// Note: serde_yaml::Error doesn't expose location() in all cases
			let response = ValidationResponse {
				valid: false,
				error: Some(format!("Parse error at '{}': {}", path, error_msg)),
				error_location: None, // Location info is included in the error message
				info: None,
			};

			return (StatusCode::OK, Json(response)).into_response();
		}
	};

	// Validate templates BEFORE normalizing (normalization clears send targets)
	if let Err(err) = validate_templates(&alert) {
		let response = ValidationResponse {
			valid: false,
			error: Some(format!("Template validation error: {:#}", err)),
			error_location: None,
			info: None,
		};

		return (StatusCode::OK, Json(response)).into_response();
	}

	// Try to normalize the alert (this validates send targets and other fields)
	let external_targets = std::collections::HashMap::new();
	match alert.normalise(&external_targets) {
		Ok((alert, resolved_targets)) => {
			let source_type = match &alert.source {
				TicketSource::Sql { .. } => "sql",
				TicketSource::Shell { .. } => "shell",
				TicketSource::Event { .. } => "event",
				TicketSource::None => "none",
			}
			.to_string();

			let response = ValidationResponse {
				valid: true,
				error: None,
				error_location: None,
				info: Some(ValidationInfo {
					enabled: alert.enabled,
					interval: alert.interval.clone(),
					source_type,
					targets: resolved_targets.len(),
				}),
			};

			(StatusCode::OK, Json(response)).into_response()
		}
		Err(err) => {
			// Normalization error (e.g., invalid interval, missing targets)
			let response = ValidationResponse {
				valid: false,
				error: Some(format!("Validation error: {:#}", err)),
				error_location: None,
				info: None,
			};

			(StatusCode::OK, Json(response)).into_response()
		}
	}
}

fn validate_templates(alert: &AlertDefinition) -> miette::Result<()> {
	use crate::templates;
	use miette::Context as _;

	// Validate each send target's templates by compiling them
	// We only compile, not render, because we don't know the actual data structure
	// that will be available at runtime (e.g., SQL column names, shell output format)
	// Compilation catches syntax errors, which is the main goal
	for (idx, target) in alert.send.iter().enumerate() {
		// Load and compile templates for this target
		// This will catch syntax errors like mismatched tags, invalid filters, etc.
		templates::load_templates(target.subject(), target.template())
			.wrap_err_with(|| format!("validating templates for send target #{}", idx + 1))?;
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	use axum::{http::StatusCode, response::IntoResponse};

	use super::*;

	#[tokio::test]
	async fn test_validate_valid_sql_alert() {
		let yaml = r#"
sql: "SELECT 1"
send:
  - id: test
    subject: Test
    template: Test
"#;

		let response = handle_validate(yaml.to_string()).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let validation: ValidationResponse = serde_json::from_slice(&body).unwrap();

		assert!(validation.valid);
		assert!(validation.info.is_some());
	}

	#[tokio::test]
	async fn test_validate_valid_shell_alert() {
		let yaml = r#"
shell: uptime
run: uptime
send:
  - id: test
    subject: Test
    template: Test
"#;

		let response = handle_validate(yaml.to_string()).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let validation: ValidationResponse = serde_json::from_slice(&body).unwrap();

		assert!(validation.valid);
		assert!(validation.info.is_some());
	}

	#[tokio::test]
	async fn test_validate_event_alert() {
		let yaml = r#"
event: http
send:
  - id: test
    subject: Test
    template: Test
"#;

		let response = handle_validate(yaml.to_string()).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let validation: ValidationResponse = serde_json::from_slice(&body).unwrap();

		assert!(validation.valid);
		assert!(validation.info.is_some());
	}

	#[tokio::test]
	async fn test_validate_invalid_yaml() {
		let yaml = "this is: not: valid: yaml:";

		let response = handle_validate(yaml.to_string()).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let validation: ValidationResponse = serde_json::from_slice(&body).unwrap();

		assert!(!validation.valid);
		assert!(validation.error.is_some());
	}

	#[tokio::test]
	async fn test_validate_template_syntax_error() {
		let yaml = r#"
sql: "SELECT 1"
send:
  - id: test
    subject: Test
    template: "{{ unclosed tag"
"#;

		let response = handle_validate(yaml.to_string()).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let validation: ValidationResponse = serde_json::from_slice(&body).unwrap();

		assert!(!validation.valid);
		assert!(validation.error.is_some());
		assert!(validation.error.unwrap().contains("Template"));
	}

	#[tokio::test]
	async fn test_validate_template_mismatched_tags() {
		let yaml = r#"
sql: "SELECT 1"
send:
  - id: test
    subject: Test
    template: "{% if foo %}bar"
"#;

		let response = handle_validate(yaml.to_string()).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let validation: ValidationResponse = serde_json::from_slice(&body).unwrap();

		assert!(!validation.valid);
		assert!(validation.error.is_some());
	}

	#[tokio::test]
	async fn test_validate_multiple_targets() {
		let yaml = r#"
sql: "SELECT 1"
send:
  - id: test1
    subject: Test 1
    template: Test 1
  - id: test2
    subject: Test 2
    template: Test 2
"#;

		let response = handle_validate(yaml.to_string()).await.into_response();

		assert_eq!(response.status(), StatusCode::OK);
		let body = axum::body::to_bytes(response.into_body(), usize::MAX)
			.await
			.unwrap();
		let validation: ValidationResponse = serde_json::from_slice(&body).unwrap();

		assert!(validation.valid);
		let info = validation.info.as_ref().unwrap();
		// Should have 0 targets because we don't provide external targets
		assert_eq!(info.targets, 0);
	}
}
