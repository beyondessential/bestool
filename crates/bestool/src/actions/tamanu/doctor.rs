use std::{io::Write, path::Path, sync::Arc};

use clap::Parser;
use miette::{IntoDiagnostic, Result, miette};
use node_semver::Version;
use owo_colors::OwoColorize;
use serde_json::{Map, Value};
use tracing::warn;

use bestool_tamanu::{
	config::{TamanuConfig, load_config},
	connection_url::ConnectionUrlBuilder,
	doctor::{
		check::{Check, CheckStatus, OverallResult},
		checks::{self, CheckContext},
		server_info::{self, ServerFacts},
	},
	server_info::get_or_create_server_id,
};

use super::{TamanuArgs, find_tamanu};
use crate::actions::Context;

/// Gather server info + healthchecks for a Tamanu install
///
/// Runs a set of healthchecks against the local Tamanu install and renders a
/// colour-coded summary. The alertd daemon runs the same checks every minute
/// and pushes results to Canopy; this command is for interactive operator use.
///
/// Exit code 0 on HEALTHY or DEGRADED, 1 on FAILING.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct DoctorArgs {
	/// Emit the JSON wire payload instead of the human-readable render
	#[arg(long)]
	pub json: bool,

	/// Run only the named check(s). Repeatable. Defaults to all.
	#[arg(long = "check", value_name = "NAME")]
	pub only: Vec<String>,
}

pub async fn run(args: DoctorArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();
	let use_colours = tamanu.use_colours;

	let (version, root) = find_tamanu(tamanu)?;
	let config = Arc::new(load_config(&root, None)?);

	let database_url = build_database_url(&config);
	let http_client = reqwest::Client::new();

	let sweep = perform_sweep(
		&version,
		&root,
		config,
		&database_url,
		http_client,
		&args.only,
		None,
	)
	.await?;

	if args.json {
		let stdout = std::io::stdout();
		let mut out = stdout.lock();
		serde_json::to_writer_pretty(&mut out, &sweep.payload).into_diagnostic()?;
		writeln!(out).into_diagnostic()?;
	} else {
		let stdout = std::io::stdout();
		let mut out = stdout.lock();
		render(
			&mut out,
			sweep.server_id.as_deref(),
			&sweep.results,
			sweep.overall,
			use_colours,
		)
		.into_diagnostic()?;
	}

	if sweep.overall == OverallResult::Failing {
		std::process::exit(1);
	}
	Ok(())
}

pub(super) struct SweepResult {
	pub server_id: Option<String>,
	pub results: Vec<(Check, bool)>,
	pub overall: OverallResult,
	pub payload: Value,
	/// `SELECT version()` result observed during this sweep, available so
	/// callers (e.g. the daemon plugin) can cache it across ticks instead of
	/// re-querying every minute.
	pub pg_version: Option<String>,
}

pub(super) fn build_database_url(config: &TamanuConfig) -> String {
	ConnectionUrlBuilder {
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
	}
	.build()
}

pub(super) async fn perform_sweep(
	version: &Version,
	root: &Path,
	config: Arc<TamanuConfig>,
	database_url: &str,
	http_client: reqwest::Client,
	selected_names: &[String],
	cached_pg_version: Option<String>,
) -> Result<SweepResult> {
	// Open a single connection up-front. Checks that need the DB share it; the
	// `db_connect` check separately measures the open latency for reporting.
	let db = match tokio_postgres::connect(database_url, tokio_postgres::NoTls).await {
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

	let check_ctx = CheckContext {
		tamanu_version: version.clone(),
		tamanu_root: root.to_path_buf(),
		config: config.clone(),
		database_url: database_url.to_owned(),
		db: db.clone(),
		http_client,
	};

	let registry = checks::all();
	let selected: Vec<&checks::CheckEntry> = if selected_names.is_empty() {
		registry.iter().collect()
	} else {
		registry
			.iter()
			.filter(|e| selected_names.iter().any(|n| n == e.name))
			.collect()
	};

	if !selected_names.is_empty() && selected.len() != selected_names.len() {
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

	let facts = collect_server_facts(&config, db.as_deref(), cached_pg_version).await;
	let pg_version = facts.pg_version.clone();
	// `env!("CARGO_PKG_VERSION")` here resolves at *this* crate's compile time
	// — the bestool crate — which is what we want in the wire payload. The
	// same expression inside `bestool-tamanu` resolves to that library's
	// version (0.1.x) and gave the wrong answer before this argument existed.
	let info =
		server_info::gather(env!("CARGO_PKG_VERSION"), &version.to_string(), facts).await;
	let info_value = serde_json::to_value(&info).into_diagnostic()?;

	let overall =
		OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
	let payload = build_payload(&info_value, &results, overall);

	Ok(SweepResult {
		server_id,
		results,
		overall,
		payload,
		pg_version,
	})
}

async fn collect_server_facts(
	config: &TamanuConfig,
	db: Option<&tokio_postgres::Client>,
	cached_pg_version: Option<String>,
) -> ServerFacts {
	let mut facts = ServerFacts {
		canonical_url: config.canonical_url().map(|u| u.to_string()),
		timezone: config.primary_time_zone().map(|s| s.to_string()),
		pg_version: cached_pg_version,
		..Default::default()
	};

	let Some(client) = db else {
		return facts;
	};

	if facts.pg_version.is_none() {
		match client.query_one("SELECT version()", &[]).await {
			Ok(row) => match row.try_get::<_, String>(0) {
				Ok(v) => facts.pg_version = Some(v),
				Err(err) => warn!("decoding pg_version: {err}"),
			},
			Err(err) => warn!("SELECT version() failed: {err}"),
		}
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

fn build_payload(info: &Value, results: &[(Check, bool)], overall: OverallResult) -> Value {
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
		let tag_coloured = match &check.status {
			CheckStatus::Pass => colour_pass(use_colours, "PASS"),
			CheckStatus::Warning(_) => {
				warnings += 1;
				colour_warn(use_colours, "WARN")
			}
			CheckStatus::Fail(_) => {
				fails += 1;
				colour_fail(use_colours, "FAIL")
			}
		};
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
		let overall =
			OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let payload = build_payload(&Value::Object(Default::default()), &results, overall);
		assert_eq!(payload["healthy"], true);
		assert_eq!(payload["health"].as_array().unwrap().len(), 2);
		assert!(payload["health"][0]["healthy"].as_bool().unwrap());
	}

	#[test]
	fn payload_warning_keeps_top_healthy_but_check_unhealthy() {
		let results = vec![pass("a"), warn("b")];
		let overall =
			OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let payload = build_payload(&Value::Object(Default::default()), &results, overall);
		assert_eq!(payload["healthy"], true);
		assert_eq!(payload["health"][1]["healthy"], false);
	}

	#[test]
	fn payload_fail_flips_top_level() {
		let results = vec![pass("a"), warn("b"), fail("c")];
		let overall =
			OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let payload = build_payload(&Value::Object(Default::default()), &results, overall);
		assert_eq!(payload["healthy"], false);
	}

	#[test]
	fn off_wire_checks_skipped_in_health_array() {
		let results = vec![
			(Check::pass("on", "ok"), true),
			(Check::pass("off", "ok"), false),
		];
		let overall =
			OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
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
		let overall =
			OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
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
		let overall =
			OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let mut buf = Vec::new();
		render(&mut buf, None, &results, overall, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert!(out.contains("FAILING"));
		assert!(out.contains("1 failed"));
	}
}
