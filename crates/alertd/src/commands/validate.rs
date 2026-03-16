use miette::{Context as _, IntoDiagnostic, NamedSource, SourceSpan};
use tracing::warn;

use super::try_connect_daemon;

/// Validate an alert definition file
pub async fn validate_alert(
	file: &std::path::Path,
	addrs: &[std::net::SocketAddr],
) -> miette::Result<()> {
	// Read the file
	let content = std::fs::read_to_string(file)
		.into_diagnostic()
		.wrap_err_with(|| format!("failed to read file: {}", file.display()))?;

	// Connect to daemon
	let (client, base_url) = try_connect_daemon(addrs).await?;

	// Check daemon version
	let status_response = client
		.get(format!("{}/status", base_url))
		.send()
		.await
		.into_diagnostic()
		.wrap_err("failed to get daemon status")?;

	#[derive(serde::Deserialize)]
	struct StatusResponse {
		version: String,
	}

	if let Ok(status) = status_response.json::<StatusResponse>().await {
		let daemon_version = &status.version;
		let cli_version = crate::VERSION;
		if daemon_version != cli_version {
			warn!(
				"version mismatch: daemon is running {} but CLI is {}",
				daemon_version, cli_version
			);
			eprintln!(
				"⚠ Warning: Version mismatch detected!\n  Daemon version: {}\n  CLI version: {}\n",
				daemon_version, cli_version
			);
		}
	}

	// Send validation request
	let response = client
		.post(format!("{}/validate", base_url))
		.body(content.clone())
		.send()
		.await
		.into_diagnostic()
		.wrap_err("failed to send validation request")?;

	if !response.status().is_success() {
		return Err(miette::miette!(
			"validation request failed with status: {}",
			response.status()
		));
	}

	// Parse response
	#[derive(serde::Deserialize)]
	struct ValidationResponse {
		valid: bool,
		error: Option<String>,
		error_location: Option<ErrorLocation>,
		info: Option<ValidationInfo>,
	}

	#[derive(serde::Deserialize)]
	struct ErrorLocation {
		line: usize,
		column: usize,
	}

	#[derive(serde::Deserialize)]
	struct ValidationInfo {
		enabled: bool,
		interval: String,
		source_type: String,
		targets: usize,
	}

	let validation: ValidationResponse = response
		.json()
		.await
		.into_diagnostic()
		.wrap_err("failed to parse validation response")?;

	if validation.valid {
		println!("✓ Alert definition is valid");
		println!("  File: {}", file.display());
		if let Some(info) = validation.info {
			println!("  Enabled: {}", info.enabled);
			println!("  Interval: {}", info.interval);
			println!("  Source: {}", info.source_type);
			println!("  Targets: {}", info.targets);

			if info.targets == 0 {
				println!("\n⚠ Warning: Alert has no resolved targets.");
				println!("  This alert may not send notifications. Check your _targets.yml file.");
			}
		}
		Ok(())
	} else {
		// Display error with source location if available
		if let Some(error_msg) = validation.error {
			if let Some(loc) = validation.error_location {
				// Calculate byte offset for miette
				let mut byte_offset = 0;
				for (idx, line_content) in content.lines().enumerate() {
					if idx + 1 < loc.line {
						byte_offset += line_content.len() + 1; // +1 for newline
					} else if idx + 1 == loc.line {
						byte_offset += loc.column.saturating_sub(1);
						break;
					}
				}

				let span_start = byte_offset;
				let span_len = content[span_start..]
					.lines()
					.next()
					.map(|l| l.len().min(80))
					.unwrap_or(1);

				Err(miette::miette!(
					labels = vec![miette::LabeledSpan::at(
						SourceSpan::new(span_start.into(), span_len),
						"here"
					)],
					"{}",
					error_msg
				)
				.with_source_code(NamedSource::new(file.display().to_string(), content)))
			} else {
				Err(miette::miette!("{}", error_msg)
					.with_source_code(NamedSource::new(file.display().to_string(), content)))
			}
		} else {
			Err(miette::miette!("validation failed with no error message"))
		}
	}
}
