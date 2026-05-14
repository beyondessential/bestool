use std::{io::Write, sync::Arc};

use clap::Parser;
use miette::{IntoDiagnostic, Result, miette};
use owo_colors::OwoColorize;
use reqwest::Url;
use serde_json::{Map, Value};
use tracing::warn;

use bestool_alertd::canopy::{CanopyClient, DEFAULT_CANOPY_URL};

use super::{
	TamanuArgs, config::load_config, connection_url::ConnectionUrlBuilder, find_tamanu,
	server_info::{fetch_device_key, get_or_create_server_id},
};
use crate::actions::Context;

pub mod check;
pub mod checks;
pub mod server_info;

use check::{Check, CheckStatus, OverallResult};
use checks::CheckContext;
use server_info::ServerFacts;

fn default_canopy_url() -> Url {
	DEFAULT_CANOPY_URL.parse().expect("default canopy URL is valid")
}

/// Gather server info + healthchecks for a Tamanu install
///
/// Runs a set of healthchecks against the local Tamanu install and renders a
/// colour-coded summary. With `--send`, also POSTs the result to Canopy at
/// `/status/{server_id}`.
///
/// Exit code 0 on HEALTHY or DEGRADED, 1 on FAILING.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct DoctorArgs {
	/// POST the result to Canopy after rendering locally
	#[arg(long)]
	pub send: bool,

	/// Canopy base URL (mTLS path)
	#[arg(long, default_value_t = default_canopy_url())]
	pub canopy_url: Url,

	/// Emit the JSON wire payload instead of the human-readable render
	#[arg(long)]
	pub json: bool,

	/// Run only the named check(s). Repeatable. Defaults to all.
	#[arg(long = "check", value_name = "NAME")]
	pub only: Vec<String>,
}

pub async fn run(ctx: Context<TamanuArgs, DoctorArgs>) -> Result<()> {
	let use_colours = ctx.args_top.use_colours;
	let args = ctx.args_sub.clone();

	let (version, root) = find_tamanu(&ctx.args_top)?;
	let config = load_config(&root, None)?;

	let builder = ConnectionUrlBuilder {
		username: config.db.username.clone(),
		password: Some(config.db.password.clone()),
		host: config
			.db
			.host
			.clone()
			.unwrap_or_else(|| "localhost".to_string()),
		port: config.db.port,
		database: config.db.name.clone(),
		ssl_mode: None,
	};
	let database_url = builder.build();

	// Open a single connection up-front. Checks that need the DB share it; the
	// `db_connect` check separately measures the open latency for reporting.
	let db = match tokio_postgres::connect(&database_url, tokio_postgres::NoTls).await {
		Ok((client, conn)) => {
			tokio::spawn(async move {
				if let Err(err) = conn.await {
					warn!("doctor db connection error: {err}");
				}
			});
			Some(Arc::new(client))
		}
		Err(_) => None,
	};

	let config = Arc::new(config);
	let check_ctx = CheckContext {
		tamanu_version: version.clone(),
		tamanu_root: root.clone(),
		config: config.clone(),
		database_url: database_url.clone(),
		db: db.clone(),
	};

	let registry = checks::all();
	let selected: Vec<&checks::CheckEntry> = if args.only.is_empty() {
		registry.iter().collect()
	} else {
		registry
			.iter()
			.filter(|e| args.only.iter().any(|n| n == e.name))
			.collect()
	};

	if !args.only.is_empty() && selected.len() != args.only.len() {
		let known: Vec<&str> = registry.iter().map(|e| e.name).collect();
		return Err(miette!(
			"unknown check name; known checks: {}",
			known.join(", ")
		));
	}

	// Run all selected checks. We could parallelise, but DB checks share a
	// single client and sequential order makes the rendered output predictable.
	let mut results: Vec<(Check, bool)> = Vec::with_capacity(selected.len());
	for entry in &selected {
		let result = (entry.run)(check_ctx.clone()).await;
		results.push((result, entry.on_wire));
	}

	let server_id = match db.as_deref() {
		Some(client) => match get_or_create_server_id(client).await {
			Ok(id) => Some(id),
			Err(err) => {
				warn!("could not resolve metaServerId: {err}");
				None
			}
		},
		None => None,
	};

	let facts = collect_server_facts(&config, db.as_deref()).await;
	let info = server_info::gather(&version.to_string(), facts).await;
	let info_value = serde_json::to_value(&info).into_diagnostic()?;

	let overall = OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
	let payload = build_payload(&info_value, &results, overall);

	if args.json {
		let stdout = std::io::stdout();
		let mut out = stdout.lock();
		serde_json::to_writer_pretty(&mut out, &payload).into_diagnostic()?;
		writeln!(out).into_diagnostic()?;
	} else {
		let stdout = std::io::stdout();
		let mut out = stdout.lock();
		render(
			&mut out,
			server_id.as_deref(),
			&results,
			overall,
			use_colours,
		)
		.into_diagnostic()?;
	}

	if args.send {
		match send_to_canopy(
			&args.canopy_url,
			server_id.as_deref(),
			&payload,
			&database_url,
		)
		.await
		{
			Ok(()) => {}
			Err(err) => {
				eprintln!("canopy send failed: {err}");
			}
		}
	}

	if overall == OverallResult::Failing {
		std::process::exit(1);
	}
	Ok(())
}

async fn collect_server_facts(
	config: &super::config::TamanuConfig,
	db: Option<&tokio_postgres::Client>,
) -> ServerFacts {
	let mut facts = ServerFacts {
		canonical_url: config.canonical_url().map(|u| u.to_string()),
		timezone: config.primary_time_zone().map(|s| s.to_string()),
		..Default::default()
	};

	let Some(client) = db else {
		return facts;
	};

	match client.query_one("SELECT version()", &[]).await {
		Ok(row) => match row.try_get::<_, String>(0) {
			Ok(v) => facts.pg_version = Some(v),
			Err(err) => warn!("decoding pg_version: {err}"),
		},
		Err(err) => warn!("SELECT version() failed: {err}"),
	}

	match client
		.query_opt(
			"SELECT value FROM local_system_facts WHERE key = 'currentSyncTick'",
			&[],
		)
		.await
	{
		Ok(Some(row)) => match row.try_get::<_, String>(0) {
			Ok(tick) => facts.current_sync_tick = Some(tick),
			Err(err) => warn!("decoding currentSyncTick: {err}"),
		},
		Ok(None) => {}
		Err(err) => warn!("querying currentSyncTick: {err}"),
	}

	facts
}

fn build_payload(
	info: &Value,
	results: &[(Check, bool)],
	overall: OverallResult,
) -> Value {
	let mut payload: Map<String, Value> = match info {
		Value::Object(o) => o.clone(),
		_ => Map::new(),
	};

	let health: Vec<Value> = results
		.iter()
		.filter(|(_, on_wire)| *on_wire)
		.map(|(c, _)| c.to_wire())
		.collect();

	payload.insert("healthy".into(), overall.is_healthy_top_level().into());
	payload.insert("health".into(), Value::Array(health));

	Value::Object(payload)
}

fn render<W: Write>(
	out: &mut W,
	server_id: Option<&str>,
	results: &[(Check, bool)],
	overall: OverallResult,
	use_colours: bool,
) -> std::io::Result<()> {
	let server_id = server_id.unwrap_or("unknown");
	writeln!(out, "Tamanu doctor (server-id: {server_id})")?;
	writeln!(out)?;

	let name_width = results
		.iter()
		.map(|(c, _)| c.name.len())
		.max()
		.unwrap_or(0);

	let (mut warnings, mut fails) = (0usize, 0usize);
	for (check, _) in results {
		let (tag, tag_coloured) = match &check.status {
			CheckStatus::Pass => ("PASS", colour_pass(use_colours, "PASS")),
			CheckStatus::Warning(_) => {
				warnings += 1;
				("WARN", colour_warn(use_colours, "WARN"))
			}
			CheckStatus::Fail(_) => {
				fails += 1;
				("FAIL", colour_fail(use_colours, "FAIL"))
			}
		};
		let _ = tag;
		writeln!(
			out,
			"  {tag_coloured}    {name:<width$}   {summary}",
			name = check.name,
			width = name_width,
			summary = check.summary,
		)?;
		if let CheckStatus::Warning(r) | CheckStatus::Fail(r) = &check.status {
			let dim = if use_colours {
				format!("{}", r.dimmed())
			} else {
				r.clone()
			};
			writeln!(
				out,
				"          {empty:<width$}     {dim}",
				empty = "",
				width = name_width
			)?;
		}
	}

	writeln!(out)?;
	let label = overall.label();
	let label_coloured = match overall {
		OverallResult::Healthy => colour_pass(use_colours, label),
		OverallResult::Degraded => colour_warn(use_colours, label),
		OverallResult::Failing => colour_fail(use_colours, label),
	};
	writeln!(
		out,
		"Result: {label_coloured} ({fails} failed, {warnings} warning{plural})",
		plural = if warnings == 1 { "" } else { "s" },
	)?;
	Ok(())
}

fn colour_pass(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.green().bold())
	} else {
		s.to_string()
	}
}

fn colour_warn(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.yellow().bold())
	} else {
		s.to_string()
	}
}

fn colour_fail(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.red().bold())
	} else {
		s.to_string()
	}
}

async fn send_to_canopy(
	base_url: &Url,
	server_id: Option<&str>,
	payload: &Value,
	database_url: &str,
) -> Result<()> {
	let server_id = server_id
		.ok_or_else(|| miette!("no metaServerId available; cannot push status to canopy"))?;

	let device_key = fetch_device_key(database_url).await;
	let client = CanopyClient::new(device_key.as_deref())
		.await?
		.ok_or_else(|| {
			miette!(
				"no canopy auth available — tailscale unreachable and no deviceKey in local_system_facts"
			)
		})?;

	client.post_status(base_url, server_id, payload).await
}

#[cfg(test)]
mod tests {
	use super::*;

	fn pass(name: &'static str) -> (Check, bool) {
		(Check::pass(name, "ok"), true)
	}
	fn warn(name: &'static str) -> (Check, bool) {
		(Check::warning(name, "deg", "reason"), true)
	}
	fn fail(name: &'static str) -> (Check, bool) {
		(Check::fail(name, "bad", "reason"), true)
	}

	#[test]
	fn payload_all_pass_is_healthy() {
		let results = vec![pass("a"), pass("b")];
		let overall = OverallResult::from_checks(
			&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
		);
		let payload = build_payload(&Value::Object(Default::default()), &results, overall);
		assert_eq!(payload["healthy"], true);
		assert_eq!(payload["health"].as_array().unwrap().len(), 2);
		assert!(payload["health"][0]["healthy"].as_bool().unwrap());
	}

	#[test]
	fn payload_warning_keeps_top_healthy_but_check_unhealthy() {
		let results = vec![pass("a"), warn("b")];
		let overall = OverallResult::from_checks(
			&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
		);
		let payload = build_payload(&Value::Object(Default::default()), &results, overall);
		assert_eq!(payload["healthy"], true);
		assert_eq!(payload["health"][1]["healthy"], false);
	}

	#[test]
	fn payload_fail_flips_top_level() {
		let results = vec![pass("a"), warn("b"), fail("c")];
		let overall = OverallResult::from_checks(
			&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
		);
		let payload = build_payload(&Value::Object(Default::default()), &results, overall);
		assert_eq!(payload["healthy"], false);
	}

	#[test]
	fn off_wire_checks_skipped_in_health_array() {
		let results = vec![
			(Check::pass("on", "ok"), true),
			(Check::pass("off", "ok"), false),
		];
		let overall = OverallResult::from_checks(
			&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
		);
		let payload = build_payload(&Value::Object(Default::default()), &results, overall);
		let names: Vec<&str> = payload["health"]
			.as_array()
			.unwrap()
			.iter()
			.map(|v| v["check"].as_str().unwrap())
			.collect();
		assert_eq!(names, vec!["on"]);
	}

	#[test]
	fn render_plain_contains_summary_line() {
		let results = vec![pass("a"), warn("b")];
		let overall = OverallResult::from_checks(
			&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
		);
		let mut buf = Vec::new();
		render(&mut buf, Some("sid-1"), &results, overall, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert!(out.contains("sid-1"));
		assert!(out.contains("PASS"));
		assert!(out.contains("WARN"));
		assert!(out.contains("DEGRADED"));
		assert!(out.contains("1 warning"));
	}

	#[test]
	fn render_failing_summary() {
		let results = vec![fail("a")];
		let overall = OverallResult::from_checks(
			&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>(),
		);
		let mut buf = Vec::new();
		render(&mut buf, None, &results, overall, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert!(out.contains("FAILING"));
		assert!(out.contains("1 failed"));
	}
}
