use std::{
	io::{IsTerminal as _, Write},
	sync::Arc,
};

use clap::Parser;
use miette::{IntoDiagnostic, Result, WrapErr, miette};
use owo_colors::OwoColorize;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use bestool_alertd::doctor::{
	SweepResult, SweepTamanu,
	check::{Check, CheckStatus, OverallResult},
	checks,
	overall_from_payload, perform_sweep,
	progress::DoctorEvent,
};
use bestool_tamanu::config::load_config;

use super::{TamanuArgs, try_find_tamanu};
use crate::actions::Context;

/// Gather server info + healthchecks for a Tamanu install
///
/// If the alertd daemon is running on this host (with its HTTP server bound to
/// the default localhost port), the most recently computed sweep is fetched
/// from it and rendered, with a note saying when those checks were actually
/// computed. Otherwise — or with `--fresh` / `--no-daemon` — the checks are
/// run locally.
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

	/// Force a fresh sweep. With alertd running, asks the daemon to recompute
	/// and streams the results back as they come in; without alertd, runs the
	/// checks locally exactly like before.
	#[arg(long)]
	pub fresh: bool,

	/// Skip the alertd integration entirely and always compute locally.
	///
	/// Combined with `--fresh` this is a no-op (a local sweep is always fresh).
	#[arg(long)]
	pub no_daemon: bool,
}

/// Where the displayed sweep came from.
enum SweepSource {
	/// Daemon's last periodic sweep — include `computed_at` so we can warn how
	/// old it is in the rendered output.
	DaemonCached { computed_at: jiff::Timestamp },
	/// Daemon-driven fresh recompute, streamed back over the task endpoint.
	DaemonStreamed,
	/// Sweep we ran ourselves.
	Local,
}

const DAEMON_BASE: &str = "http://127.0.0.1:8271";

pub async fn run(args: DoctorArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();
	let use_colours = tamanu.use_colours;

	// `None` when this host has no Tamanu: the sweep still runs, with every
	// Tamanu-dependent check skipped.
	let install = match try_find_tamanu(tamanu).await? {
		Some((version, root)) => {
			let config = Arc::new(load_config(&root, None)?);
			let database_url = config.database_url();
			Some(SweepTamanu {
				version,
				root,
				config,
				database_url,
			})
		}
		None => {
			warn!("no Tamanu on this host; running host-level checks only");
			None
		}
	};
	let http_client = crate::http::client();

	let (sweep, source) = if args.no_daemon {
		(
			run_local_sweep(install.clone(), http_client.clone(), &args, use_colours).await?,
			SweepSource::Local,
		)
	} else if args.fresh {
		match run_daemon_recompute(&http_client, &args, use_colours).await {
			Ok(sweep) => (sweep, SweepSource::DaemonStreamed),
			Err(err) => {
				debug!(%err, "daemon recompute unavailable, falling back to local");
				(
					run_local_sweep(install.clone(), http_client.clone(), &args, use_colours)
						.await?,
					SweepSource::Local,
				)
			}
		}
	} else {
		match fetch_daemon_latest(&http_client).await {
			Ok((sweep, computed_at)) => (sweep, SweepSource::DaemonCached { computed_at }),
			Err(err) => {
				debug!(%err, "daemon latest unavailable, falling back to local");
				(
					run_local_sweep(install.clone(), http_client.clone(), &args, use_colours)
						.await?,
					SweepSource::Local,
				)
			}
		}
	};

	render_final(&args, &sweep, &source, use_colours)?;

	if sweep.overall == OverallResult::Failing {
		std::process::exit(1);
	}
	Ok(())
}

async fn run_local_sweep(
	install: Option<SweepTamanu>,
	http_client: reqwest::Client,
	args: &DoctorArgs,
	use_colours: bool,
) -> Result<SweepResult> {
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
		env!("CARGO_PKG_VERSION"),
		install,
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

	Ok(sweep)
}

fn render_final(
	args: &DoctorArgs,
	sweep: &SweepResult,
	source: &SweepSource,
	use_colours: bool,
) -> Result<()> {
	let stdout = std::io::stdout();
	let mut out = stdout.lock();
	let live = !args.json && std::io::stdout().is_terminal();

	if args.json {
		// Embed source info alongside the payload so JSON consumers can tell
		// where the data came from. The original payload becomes the inner
		// `wire` field; cached daemon reads also carry `computedAt`.
		let mut wrapped = serde_json::Map::new();
		wrapped.insert("wire".into(), sweep.payload.clone());
		match source {
			SweepSource::Local => {
				wrapped.insert("source".into(), Value::String("local".into()));
			}
			SweepSource::DaemonStreamed => {
				wrapped.insert("source".into(), Value::String("daemon-streamed".into()));
			}
			SweepSource::DaemonCached { computed_at } => {
				wrapped.insert("source".into(), Value::String("daemon-cached".into()));
				wrapped.insert("computedAt".into(), Value::String(computed_at.to_string()));
			}
		}
		serde_json::to_writer_pretty(&mut out, &Value::Object(wrapped)).into_diagnostic()?;
		writeln!(out).into_diagnostic()?;
		return Ok(());
	}

	if live {
		// In live mode the per-check render already happened. Just append the
		// summary + source note.
		write_source_note(&mut out, source, use_colours).into_diagnostic()?;
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
		write_source_note(&mut out, source, use_colours).into_diagnostic()?;
	}
	Ok(())
}

fn write_source_note<W: Write>(
	out: &mut W,
	source: &SweepSource,
	use_colours: bool,
) -> std::io::Result<()> {
	let line = match source {
		SweepSource::Local => return Ok(()),
		SweepSource::DaemonStreamed => "Source: alertd daemon (just now, on demand)".to_string(),
		SweepSource::DaemonCached { computed_at } => {
			let age = humanise_age_since(*computed_at);
			format!("Source: alertd daemon (computed {age} ago, at {computed_at})")
		}
	};
	if use_colours {
		writeln!(out, "{}", line.dimmed())
	} else {
		writeln!(out, "{line}")
	}
}

fn humanise_age_since(then: jiff::Timestamp) -> String {
	let now = jiff::Timestamp::now();
	let secs = now.as_second().saturating_sub(then.as_second()).max(0) as u64;
	if secs < 60 {
		format!("{secs}s")
	} else if secs < 3600 {
		format!("{}m {}s", secs / 60, secs % 60)
	} else {
		format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
	}
}

/// Read the alertd daemon's most recent sweep over `/tasks/doctor/latest`.
///
/// The short timeout is intentional: this is run on every `tamanu doctor`
/// invocation, and if alertd isn't on this host (or its HTTP server is
/// missing) we want to bail out fast and fall back to a local sweep.
async fn fetch_daemon_latest(
	http: &reqwest::Client,
) -> Result<(SweepResult, jiff::Timestamp)> {
	let url = format!("{DAEMON_BASE}/tasks/doctor/latest");
	let response = http
		.get(&url)
		.timeout(std::time::Duration::from_secs(3))
		.send()
		.await
		.into_diagnostic()
		.wrap_err("contacting local alertd")?;

	if !response.status().is_success() {
		return Err(miette!(
			"alertd /tasks/doctor/latest returned {}",
			response.status()
		));
	}

	let payload: Value = response
		.json()
		.await
		.into_diagnostic()
		.wrap_err("decoding alertd latest payload")?;
	let computed_at: jiff::Timestamp = payload
		.get("computedAt")
		.and_then(Value::as_str)
		.ok_or_else(|| miette!("alertd latest payload missing computedAt"))?
		.parse()
		.into_diagnostic()
		.wrap_err("parsing computedAt timestamp")?;

	let inner = payload
		.get("payload")
		.cloned()
		.ok_or_else(|| miette!("alertd latest payload missing payload"))?;
	let server_id = payload
		.get("serverId")
		.and_then(Value::as_str)
		.map(str::to_string);

	let overall = overall_from_payload(&inner);
	Ok((
		SweepResult {
			server_id,
			results: Vec::new(),
			overall,
			payload: inner,
			pg_version: None,
		},
		computed_at,
	))
}

/// Drive a fresh sweep on the daemon and stream the per-check results back.
///
/// Each NDJSON line is a `{"event": ...}` object; check events feed the same
/// live renderer that local sweeps use, the final `done` event carries the
/// full payload to render the summary off.
async fn run_daemon_recompute(
	http: &reqwest::Client,
	args: &DoctorArgs,
	use_colours: bool,
) -> Result<SweepResult> {
	let url = format!("{DAEMON_BASE}/tasks/doctor/recompute");
	let response = http
		.get(&url)
		.timeout(std::time::Duration::from_secs(5))
		.send()
		.await
		.into_diagnostic()
		.wrap_err("contacting local alertd")?;

	if !response.status().is_success() {
		return Err(miette!(
			"alertd /tasks/doctor/recompute returned {}",
			response.status()
		));
	}

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

	let registry = checks::all();
	let resolve_name = |s: &str| {
		registry
			.iter()
			.find(|e| e.name == s)
			.map(|e| e.name)
	};

	let mut stream = response.bytes_stream();
	let mut buffer = Vec::<u8>::new();
	let mut final_payload: Option<Value> = None;
	let mut server_id: Option<String> = None;

	use futures::StreamExt as _;
	while let Some(chunk) = stream.next().await {
		let chunk = chunk
			.into_diagnostic()
			.wrap_err("reading alertd recompute stream")?;
		buffer.extend_from_slice(&chunk);
		while let Some(nl) = buffer.iter().position(|&b| b == b'\n') {
			let line: Vec<u8> = buffer.drain(..=nl).collect();
			let line = &line[..line.len() - 1];
			if line.is_empty() {
				continue;
			}
			let value: Value = match serde_json::from_slice(line) {
				Ok(v) => v,
				Err(err) => {
					warn!(%err, "could not parse alertd recompute line");
					continue;
				}
			};
			match value.get("event").and_then(Value::as_str) {
				Some("check") => {
					if let Some(check_json) = value.get("check")
						&& let Some(check) = Check::from_streaming_json(check_json, resolve_name)
						&& let Some((tx, _)) = &renderer
					{
						let _ = tx.send(DoctorEvent::Completed(check));
					}
				}
				Some("done") => {
					final_payload = value.get("payload").cloned();
					server_id = value
						.get("serverId")
						.and_then(Value::as_str)
						.map(str::to_string);
				}
				Some("error") => {
					let msg = value
						.get("message")
						.and_then(Value::as_str)
						.unwrap_or("unknown");
					return Err(miette!("alertd recompute reported error: {msg}"));
				}
				_ => {}
			}
		}
	}

	if let Some((tx, handle)) = renderer {
		drop(tx);
		let _ = handle.await;
	}

	let payload = final_payload
		.ok_or_else(|| miette!("alertd recompute stream ended without a done event"))?;
	let overall = overall_from_payload(&payload);
	Ok(SweepResult {
		server_id,
		results: Vec::new(),
		overall,
		payload,
		pg_version: None,
	})
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
		CheckStatus::Broken(_) => colour_broken(use_colours, "BRKN"),
	};
	writeln!(
		out,
		"  {tag_coloured}    {name:<width$}   {summary}",
		name = check.name,
		width = name_width,
		summary = check.summary,
	)?;
	if let CheckStatus::Skip(r)
	| CheckStatus::Warning(r)
	| CheckStatus::Fail(r)
	| CheckStatus::Broken(r) = &check.status
	{
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
	let (mut warnings, mut fails, mut skips, mut brokens) = (0usize, 0usize, 0usize, 0usize);
	for (check, _) in results {
		match &check.status {
			CheckStatus::Pass => {}
			CheckStatus::Skip(_) => skips += 1,
			CheckStatus::Warning(_) => warnings += 1,
			CheckStatus::Fail(_) => fails += 1,
			CheckStatus::Broken(_) => brokens += 1,
		}
	}
	let label = overall.label();
	let label_coloured = match overall {
		OverallResult::Healthy => colour_pass(use_colours, label),
		OverallResult::Degraded => colour_warn(use_colours, label),
		OverallResult::Failing => colour_fail(use_colours, label),
	};
	let broken_suffix = if brokens > 0 {
		format!(", {brokens} broken")
	} else {
		String::new()
	};
	let skip_suffix = if skips > 0 {
		format!(", {skips} skipped")
	} else {
		String::new()
	};
	writeln!(
		out,
		"Result: {label_coloured} ({fails} failed, {warnings} warning{plural}{broken_suffix}{skip_suffix})",
		plural = if warnings == 1 { "" } else { "s" },
	)
}

/// Streams check results to `out` as they come in over `rx`, with a rewriting
/// "Outstanding: ..." line below the printed results. The outstanding line is
/// truncated to the terminal width so `\r\x1b[2K` reliably erases it on the
/// next update; without that, terminal wrapping leaves orphaned rows behind
/// as the cursor only sits on the last wrapped row.
fn render_live<W: Write>(
	out: &mut W,
	selected_names: &[&'static str],
	mut rx: mpsc::UnboundedReceiver<DoctorEvent>,
	use_colours: bool,
) -> std::io::Result<()> {
	let name_width = selected_names.iter().map(|n| n.len()).max().unwrap_or(0);
	let term_width = terminal_size::terminal_size()
		.map(|(terminal_size::Width(w), _)| w)
		.unwrap_or(80);
	let mut outstanding: Vec<&'static str> = selected_names.to_vec();

	write_outstanding(out, &outstanding, term_width, use_colours)?;
	out.flush()?;

	while let Some(event) = rx.blocking_recv() {
		match event {
			DoctorEvent::Completed(check) => {
				clear_current_line(out)?;
				write_check_line(out, &check, name_width, use_colours)?;
				outstanding.retain(|n| *n != check.name);
				write_outstanding(out, &outstanding, term_width, use_colours)?;
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
	term_width: u16,
	use_colours: bool,
) -> std::io::Result<()> {
	if outstanding.is_empty() {
		return Ok(());
	}
	let plain = format!("Outstanding: {}", outstanding.join(", "));
	let truncated = truncate_to_width(&plain, term_width);
	if use_colours {
		write!(out, "{}", truncated.dimmed())
	} else {
		write!(out, "{truncated}")
	}
}

/// Truncate `s` to fit within `width` display columns, appending `…` when the
/// string is cut. Treats each char as one column — fine for the ASCII-only
/// check names this is used with.
fn truncate_to_width(s: &str, width: u16) -> String {
	let width = width as usize;
	if width == 0 {
		return String::new();
	}
	if s.chars().count() <= width {
		return s.to_string();
	}
	if width == 1 {
		return "…".to_string();
	}
	let take = width - 1;
	let mut out: String = s.chars().take(take).collect();
	out.push('…');
	out
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

fn colour_broken(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.magenta().bold())
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
	fn truncate_to_width_pads_short_strings_unchanged() {
		assert_eq!(truncate_to_width("abc", 10), "abc");
	}

	#[test]
	fn truncate_to_width_chops_with_ellipsis() {
		assert_eq!(truncate_to_width("abcdefghij", 5), "abcd…");
	}

	#[test]
	fn truncate_to_width_handles_exact_fit() {
		assert_eq!(truncate_to_width("abcde", 5), "abcde");
	}

	#[test]
	fn write_outstanding_truncates_to_one_terminal_row() {
		let mut buf = Vec::new();
		let names = ["alpha", "bravo", "charlie", "delta", "echo", "foxtrot"];
		write_outstanding(&mut buf, &names, 30, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert_eq!(out.chars().count(), 30);
		assert!(out.ends_with('…'));
		assert!(out.starts_with("Outstanding: "));
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
