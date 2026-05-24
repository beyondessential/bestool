use std::{
	io::{IsTerminal as _, Write},
	path::Path,
	sync::Arc,
};

use clap::Parser;
use futures::stream::{FuturesUnordered, StreamExt};
use miette::{IntoDiagnostic, Result, miette};
use node_semver::Version;
use owo_colors::OwoColorize;
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tracing::warn;

use bestool_tamanu::{
	config::{TamanuConfig, load_config},
	connection_url::ConnectionUrlBuilder,
	doctor::{
		check::{Check, CheckStatus, OverallResult},
		checks::{self, CheckContext},
		progress::{DoctorEvent, ProgressSender},
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

	/// Skip the named check(s). Repeatable. Applied after `--check`.
	#[arg(long = "skip", value_name = "NAME")]
	pub skip: Vec<String>,
}

pub async fn run(args: DoctorArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();
	let use_colours = tamanu.use_colours;

	let (version, root) = find_tamanu(tamanu)?;
	let config = Arc::new(load_config(&root, None)?);

	let database_url = build_database_url(&config);
	let http_client = reqwest::Client::new();

	let live = !args.json && std::io::stdout().is_terminal();
	let selected_names = selected_names_for_render(&args.only, &args.skip)?;
	let renderer = if live {
		let (tx, rx) = mpsc::unbounded_channel();
		let names = selected_names.clone();
		let handle = tokio::task::spawn_blocking(move || {
			let stdout = std::io::stdout();
			let mut out = stdout.lock();
			let _ = render_live(&mut out, &names, rx, use_colours);
		});
		Some((tx, handle))
	} else {
		None
	};

	let progress = renderer.as_ref().map(|(tx, _)| tx.clone());
	let sweep = perform_sweep(
		&version,
		&root,
		config,
		&database_url,
		http_client,
		&args.only,
		&args.skip,
		None,
		progress,
	)
	.await?;

	if let Some((tx, handle)) = renderer {
		drop(tx);
		let _ = handle.await;
	}

	if args.json {
		let stdout = std::io::stdout();
		let mut out = stdout.lock();
		serde_json::to_writer_pretty(&mut out, &sweep.payload).into_diagnostic()?;
		writeln!(out).into_diagnostic()?;
	} else {
		let stdout = std::io::stdout();
		let mut out = stdout.lock();
		if live {
			render_summary(
				&mut out,
				sweep.server_id.as_deref(),
				&sweep.results,
				sweep.overall,
				use_colours,
			)
			.into_diagnostic()?;
		} else {
			render(
				&mut out,
				sweep.server_id.as_deref(),
				&sweep.results,
				sweep.overall,
				use_colours,
			)
			.into_diagnostic()?;
		}
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

#[expect(
	clippy::too_many_arguments,
	reason = "each argument is a distinct knob the CLI and daemon callers need to thread through"
)]
pub(super) async fn perform_sweep(
	version: &Version,
	root: &Path,
	config: Arc<TamanuConfig>,
	database_url: &str,
	http_client: reqwest::Client,
	selected_names: &[String],
	skip_names: &[String],
	cached_pg_version: Option<String>,
	progress: Option<ProgressSender>,
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
	let known: Vec<&str> = registry.iter().map(|e| e.name).collect();
	if let Some(unknown) = selected_names.iter().find(|n| !known.contains(&n.as_str())) {
		return Err(miette!(
			"unknown check name `{unknown}`; known checks: {}",
			known.join(", ")
		));
	}
	if let Some(unknown) = skip_names.iter().find(|n| !known.contains(&n.as_str())) {
		return Err(miette!(
			"unknown check name `{unknown}` in --skip; known checks: {}",
			known.join(", ")
		));
	}

	let selected: Vec<(usize, &checks::CheckEntry)> = registry
		.iter()
		.enumerate()
		.filter(|(_, e)| selected_names.is_empty() || selected_names.iter().any(|n| n == e.name))
		.filter(|(_, e)| !skip_names.iter().any(|n| n == e.name))
		.collect();

	// Run all selected checks concurrently. Results are collated by registry
	// index before returning, so callers see a stable order regardless of
	// completion order. A progress channel can observe results as they land.
	let mut pending = FuturesUnordered::new();
	for (idx, entry) in &selected {
		let ctx = check_ctx.clone();
		let on_wire = entry.on_wire;
		let idx = *idx;
		let fut = (entry.run)(ctx);
		pending.push(async move {
			let result = fut.await;
			(idx, on_wire, result)
		});
	}

	let mut completed: Vec<(usize, Check, bool)> = Vec::with_capacity(selected.len());
	while let Some((idx, on_wire, check)) = pending.next().await {
		if let Some(tx) = progress.as_ref() {
			let _ = tx.send(DoctorEvent::Completed(check.clone()));
		}
		completed.push((idx, check, on_wire));
	}
	completed.sort_by_key(|(idx, _, _)| *idx);
	let results: Vec<(Check, bool)> = completed.into_iter().map(|(_, c, w)| (c, w)).collect();

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
	let info = server_info::gather(&version.to_string(), facts).await;
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

fn selected_names_for_render(only: &[String], skip: &[String]) -> Result<Vec<&'static str>> {
	let registry = checks::all();
	let known: Vec<&str> = registry.iter().map(|e| e.name).collect();
	if let Some(unknown) = only.iter().find(|n| !known.contains(&n.as_str())) {
		return Err(miette!(
			"unknown check name `{unknown}`; known checks: {}",
			known.join(", ")
		));
	}
	if let Some(unknown) = skip.iter().find(|n| !known.contains(&n.as_str())) {
		return Err(miette!(
			"unknown check name `{unknown}` in --skip; known checks: {}",
			known.join(", ")
		));
	}
	Ok(registry
		.iter()
		.filter(|e| only.is_empty() || only.iter().any(|n| n == e.name))
		.filter(|e| !skip.iter().any(|n| n == e.name))
		.map(|e| e.name)
		.collect())
}

fn render<W: Write>(
	out: &mut W,
	server_id: Option<&str>,
	results: &[(Check, bool)],
	overall: OverallResult,
	use_colours: bool,
) -> std::io::Result<()> {
	write_header(out, server_id)?;

	let name_width = results
		.iter()
		.map(|(c, _)| c.name.len())
		.max()
		.unwrap_or(0);

	for (check, _) in results {
		write_check_line(out, check, name_width, use_colours)?;
	}

	writeln!(out)?;
	write_result_line(out, results, overall, use_colours)?;
	Ok(())
}

fn write_header<W: Write>(out: &mut W, server_id: Option<&str>) -> std::io::Result<()> {
	let server_id = server_id.unwrap_or("unknown");
	writeln!(out, "Tamanu doctor (server-id: {server_id})")?;
	writeln!(out)
}

fn write_check_line<W: Write>(
	out: &mut W,
	check: &Check,
	name_width: usize,
	use_colours: bool,
) -> std::io::Result<()> {
	let tag_coloured = match &check.status {
		CheckStatus::Pass => colour_pass(use_colours, "PASS"),
		CheckStatus::Skip(_) => colour_skip(use_colours, "SKIP"),
		CheckStatus::Warning(_) => colour_warn(use_colours, "WARN"),
		CheckStatus::Fail(_) => colour_fail(use_colours, "FAIL"),
	};
	writeln!(
		out,
		"  {tag_coloured}    {name:<width$}   {summary}",
		name = check.name,
		width = name_width,
		summary = check.summary,
	)?;
	if let CheckStatus::Skip(r) | CheckStatus::Warning(r) | CheckStatus::Fail(r) = &check.status {
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
	Ok(())
}

fn write_result_line<W: Write>(
	out: &mut W,
	results: &[(Check, bool)],
	overall: OverallResult,
	use_colours: bool,
) -> std::io::Result<()> {
	let (mut warnings, mut fails, mut skips) = (0usize, 0usize, 0usize);
	for (check, _) in results {
		match &check.status {
			CheckStatus::Pass => {}
			CheckStatus::Skip(_) => skips += 1,
			CheckStatus::Warning(_) => warnings += 1,
			CheckStatus::Fail(_) => fails += 1,
		}
	}
	let label = overall.label();
	let label_coloured = match overall {
		OverallResult::Healthy => colour_pass(use_colours, label),
		OverallResult::Degraded => colour_warn(use_colours, label),
		OverallResult::Failing => colour_fail(use_colours, label),
	};
	let skip_suffix = if skips > 0 {
		format!(", {skips} skipped")
	} else {
		String::new()
	};
	writeln!(
		out,
		"Result: {label_coloured} ({fails} failed, {warnings} warning{plural}{skip_suffix})",
		plural = if warnings == 1 { "" } else { "s" },
	)
}

/// Streams check results to `out` as they come in over `rx`, with a rewriting
/// "Outstanding: ..." line below the printed results. Falls back gracefully if
/// `out` doesn't accept the ANSI line-erase escape (the trailing newline ensures
/// no half-erased line is left over).
fn render_live<W: Write>(
	out: &mut W,
	selected_names: &[&'static str],
	mut rx: mpsc::UnboundedReceiver<DoctorEvent>,
	use_colours: bool,
) -> std::io::Result<()> {
	let name_width = selected_names.iter().map(|n| n.len()).max().unwrap_or(0);
	let mut outstanding: Vec<&'static str> = selected_names.to_vec();

	write_outstanding(out, &outstanding, use_colours)?;
	out.flush()?;

	while let Some(event) = rx.blocking_recv() {
		match event {
			DoctorEvent::Completed(check) => {
				clear_current_line(out)?;
				write_check_line(out, &check, name_width, use_colours)?;
				outstanding.retain(|n| *n != check.name);
				write_outstanding(out, &outstanding, use_colours)?;
				out.flush()?;
			}
		}
	}

	clear_current_line(out)?;
	out.flush()
}

fn render_summary<W: Write>(
	out: &mut W,
	server_id: Option<&str>,
	results: &[(Check, bool)],
	overall: OverallResult,
	use_colours: bool,
) -> std::io::Result<()> {
	writeln!(out)?;
	let server_id = server_id.unwrap_or("unknown");
	writeln!(out, "Server: {server_id}")?;
	write_result_line(out, results, overall, use_colours)
}

fn write_outstanding<W: Write>(
	out: &mut W,
	outstanding: &[&'static str],
	use_colours: bool,
) -> std::io::Result<()> {
	if outstanding.is_empty() {
		return Ok(());
	}
	let label = if use_colours {
		format!("{}", "Outstanding:".dimmed())
	} else {
		"Outstanding:".to_string()
	};
	write!(out, "{label} {}", outstanding.join(", "))
}

fn clear_current_line<W: Write>(out: &mut W) -> std::io::Result<()> {
	// CR brings cursor to col 0; \x1b[2K erases the whole line.
	write!(out, "\r\x1b[2K")
}

fn colour_pass(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.green().bold())
	} else {
		s.to_string()
	}
}

fn colour_skip(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.dimmed().bold())
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
	fn skip(name: &'static str) -> (Check, bool) {
		(Check::skip(name, "not run", "reason"), true)
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
	fn skip_renders_as_skip_and_doesnt_degrade_overall() {
		// A skipped check should appear with the SKIP tag, not be counted as
		// a warning or failure, and keep the overall result HEALTHY.
		let results = vec![pass("a"), skip("b")];
		let overall =
			OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		assert_eq!(overall, OverallResult::Healthy);

		let mut buf = Vec::new();
		render(&mut buf, Some("sid"), &results, overall, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert!(out.contains("SKIP"));
		assert!(out.contains("HEALTHY"));
		assert!(out.contains("1 skipped"));
		// Skip should NOT bump the warning count
		assert!(!out.contains("1 warning"));
	}

	#[test]
	fn skip_is_healthy_on_wire() {
		// The whole point of distinguishing Skip from Fail/Warning is that
		// "we don't know" shouldn't fire alerts downstream of the wire format.
		let results = vec![pass("a"), skip("b")];
		let overall =
			OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let payload = build_payload(&Value::Object(Default::default()), &results, overall);
		assert_eq!(payload["healthy"], true);
		assert_eq!(payload["health"][1]["healthy"], true);
		assert_eq!(payload["health"][1]["skipped"], true);
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

	#[test]
	fn selected_names_default_returns_full_registry() {
		let names = selected_names_for_render(&[], &[]).unwrap();
		let registry: Vec<&str> = checks::all().iter().map(|e| e.name).collect();
		assert_eq!(names, registry);
	}

	#[test]
	fn selected_names_only_filters_to_listed() {
		let names =
			selected_names_for_render(&["db_connect".into(), "memory".into()], &[]).unwrap();
		assert_eq!(names, vec!["db_connect", "memory"]);
	}

	#[test]
	fn selected_names_skip_excludes_listed() {
		let names = selected_names_for_render(&[], &["tailscale".into()]).unwrap();
		assert!(!names.contains(&"tailscale"));
		assert!(names.contains(&"db_connect"));
	}

	#[test]
	fn selected_names_only_and_skip_compose() {
		let names = selected_names_for_render(
			&["db_connect".into(), "memory".into(), "tailscale".into()],
			&["tailscale".into()],
		)
		.unwrap();
		assert_eq!(names, vec!["db_connect", "memory"]);
	}

	#[test]
	fn selected_names_unknown_skip_is_error() {
		let err = selected_names_for_render(&[], &["does_not_exist".into()]).unwrap_err();
		assert!(format!("{err}").contains("does_not_exist"));
	}

	#[test]
	fn render_live_streams_results_and_clears_outstanding() {
		let (tx, rx) = mpsc::unbounded_channel();
		let names = vec!["alpha", "beta"];
		let handle = std::thread::spawn(move || {
			let mut buf = Vec::new();
			render_live(&mut buf, &names, rx, false).unwrap();
			String::from_utf8(buf).unwrap()
		});
		tx.send(DoctorEvent::Completed(Check::pass("alpha", "ok-a")))
			.unwrap();
		tx.send(DoctorEvent::Completed(Check::warning(
			"beta", "deg", "reason",
		)))
		.unwrap();
		drop(tx);
		let out = handle.join().unwrap();
		assert!(out.contains("PASS"));
		assert!(out.contains("alpha"));
		assert!(out.contains("ok-a"));
		assert!(out.contains("WARN"));
		assert!(out.contains("beta"));
		assert!(out.contains("Outstanding:"));
	}

	#[test]
	fn render_summary_includes_server_and_result() {
		let results = vec![pass("a"), warn("b")];
		let overall =
			OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let mut buf = Vec::new();
		render_summary(&mut buf, Some("sid-9"), &results, overall, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert!(out.contains("Server: sid-9"));
		assert!(out.contains("DEGRADED"));
		assert!(out.contains("1 warning"));
	}
}
