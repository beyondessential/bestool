//! Reaching the running daemon's canopy names task.
//!
//! The daemon holds the entitlement, the keys, and the collected chains, so the
//! commands ask it rather than opening their own canopy client: one place makes
//! the requests, and what a command reports is what the daemon will act on.
//!
//! spec: TLS#commands
//! spec: NAM#commands

use std::net::SocketAddr;

use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use serde_json::Value;

use crate::alertd::{certificates::TASK_NAME, commands};

/// Call one of the task's endpoints on a running daemon.
///
/// A daemon that isn't running is reported as such rather than as a bare
/// connection error: it is the thing the operator has to start.
pub async fn ask(
	addrs: &[SocketAddr],
	endpoint: &str,
    query: &[(&str, String)],
) -> Result<Value> {
	let addrs = if addrs.is_empty() {
		commands::default_server_addrs()
	} else {
		addrs.to_vec()
	};
	let (client, base) = commands::try_connect_daemon(&addrs).await.wrap_err(
		"no alertd daemon is running here; certificates are collected by the daemon, so start it first",
	)?;

	let response = client
		.get(format!("{base}/tasks/{TASK_NAME}/{endpoint}"))
		.query(query)
		.send()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("asking the daemon for {endpoint}"))?;

	let status = response.status();
	let body = response.text().await.into_diagnostic()?;
	if !status.is_success() {
		return Err(miette!("the daemon refused: {}", body.trim()));
	}
	serde_json::from_str(&body)
		.into_diagnostic()
		.wrap_err("parsing the daemon's answer")
}

/// Print a value under a heading, or say there is nothing to print.
pub fn list(heading: &str, rows: &[String]) {
	println!("{heading}:");
	if rows.is_empty() {
		println!("  (none)");
	}
	for row in rows {
		println!("  {row}");
	}
}

/// A JSON field as a string, for printing.
pub fn text(value: &Value, key: &str) -> String {
	match &value[key] {
		Value::Null => "—".to_string(),
		Value::String(s) => s.clone(),
		other => other.to_string(),
	}
}

/// A JSON array of strings, joined for printing.
pub fn joined(value: &Value, key: &str) -> String {
	value[key]
		.as_array()
		.map(|items| {
			items
				.iter()
				.filter_map(Value::as_str)
				.collect::<Vec<_>>()
				.join(", ")
		})
		.filter(|s| !s.is_empty())
		.unwrap_or_else(|| "(none)".to_string())
}
