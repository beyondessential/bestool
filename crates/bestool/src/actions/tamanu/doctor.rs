use std::io::{IsTerminal as _, Write};

use clap::Parser;
use miette::{IntoDiagnostic, Result, WrapErr, miette};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use bestool_alertd::doctor::{
	SweepResult, SweepTamanu,
	check::{Check, CheckStatus, OverallResult},
	checks,
	overall_from_payload, perform_sweep,
	progress::ProgressSender,
	resolve_sweep_tamanu,
};

use super::{TamanuArgs, try_find_tamanu};
use crate::actions::Context;

mod order;
mod render;
mod tui;

/// Gather server info + healthchecks for a Tamanu install
///
/// If the alertd daemon is running on this host (with its HTTP server bound to
/// the default localhost port), the most recently computed sweep is fetched
/// from it and rendered, with a note saying when those checks were actually
/// computed. Otherwise — or with `--fresh` / `--no-daemon` — the checks are
/// run locally.
///
/// Exit code 0 on HEALTHY or DEGRADED, 1 on FAILING, 130 on interrupt.
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

	/// Show every check in the result replay, including passing and skipped.
	///
	/// By default the replay lists only warning, broken, and failing checks; the
	/// live progress view always shows every check regardless.
	#[arg(long, short = 'a')]
	pub all: bool,

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
pub enum SweepSource {
	/// Daemon's last periodic sweep — include `computed_at` so we can note how
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

	let install = resolve_sweep_tamanu(try_find_tamanu(tamanu).await?)?;
	if install.is_none() {
		warn!("no Tamanu on this host; running host-level checks only");
	}
	let http_client = crate::http::client();

	let live_tty = !args.json && std::io::stdout().is_terminal();

	let (sweep, source, interrupted) = if args.no_daemon {
		let outcome = run_local_sweep(install.clone(), http_client.clone(), &args, live_tty).await?;
		(outcome.sweep, SweepSource::Local, outcome.interrupted)
	} else if args.fresh {
		match run_daemon_recompute(&http_client, &args, live_tty).await {
			Ok(outcome) => (outcome.sweep, SweepSource::DaemonStreamed, outcome.interrupted),
			Err(err) => {
				debug!(%err, "daemon recompute unavailable, falling back to local");
				let outcome =
					run_local_sweep(install.clone(), http_client.clone(), &args, live_tty).await?;
				(outcome.sweep, SweepSource::Local, outcome.interrupted)
			}
		}
	} else {
		match fetch_daemon_latest(&http_client).await {
			Ok((sweep, computed_at)) => (sweep, SweepSource::DaemonCached { computed_at }, false),
			Err(err) => {
				debug!(%err, "daemon latest unavailable, falling back to local");
				let outcome =
					run_local_sweep(install.clone(), http_client.clone(), &args, live_tty).await?;
				(outcome.sweep, SweepSource::Local, outcome.interrupted)
			}
		}
	};

	emit_output(&args, &sweep, &source, use_colours)?;

	if interrupted {
		std::process::exit(130);
	}
	if sweep.overall == OverallResult::Failing {
		std::process::exit(1);
	}
	Ok(())
}

struct SweepOutcome {
	sweep: SweepResult,
	interrupted: bool,
}

async fn run_local_sweep(
	install: Option<SweepTamanu>,
	http_client: reqwest::Client,
	args: &DoctorArgs,
	live_tty: bool,
) -> Result<SweepOutcome> {
	let selected_names = selected_names(&args.only, &args.skip)?;
	let (progress, tui_handle) = setup_progress(live_tty, &selected_names, SweepSource::Local);

	let sweep_args_only = args.only.clone();
	let sweep_args_skip = args.skip.clone();
	let sweep_handle = tokio::spawn(async move {
		perform_sweep(
			env!("CARGO_PKG_VERSION"),
			install,
			http_client,
			&sweep_args_only,
			&sweep_args_skip,
			None,
			progress,
		)
		.await
	});

	let interrupted = if let Some(handle) = tui_handle {
		let outcome = handle.await.into_diagnostic()??;
		if outcome.interrupted {
			sweep_handle.abort();
			let mut synthetic = synthetic_sweep(outcome.results);
			synthetic.payload = serde_json::Value::Object(Default::default());
			return Ok(SweepOutcome {
				sweep: synthetic,
				interrupted: true,
			});
		}
		false
	} else {
		false
	};

	let sweep = sweep_handle.await.into_diagnostic()??;
	Ok(SweepOutcome { sweep, interrupted })
}

/// Drive a fresh sweep on the daemon and stream the per-check results back.
async fn run_daemon_recompute(
	http: &reqwest::Client,
	args: &DoctorArgs,
	live_tty: bool,
) -> Result<SweepOutcome> {
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

	let selected_names = selected_names(&args.only, &args.skip)?;
	let (progress, tui_handle) = setup_progress(live_tty, &selected_names, SweepSource::DaemonStreamed);

	let stream_handle = tokio::spawn(drain_recompute_stream(response, progress));

	let interrupted = if let Some(handle) = tui_handle {
		let outcome = handle.await.into_diagnostic()??;
		if outcome.interrupted {
			stream_handle.abort();
			return Ok(SweepOutcome {
				sweep: synthetic_sweep(outcome.results),
				interrupted: true,
			});
		}
		false
	} else {
		false
	};

	let streamed = stream_handle.await.into_diagnostic()??;
	let overall = overall_from_payload(&streamed.payload);
	Ok(SweepOutcome {
		sweep: SweepResult {
			server_id: streamed.server_id,
			results: streamed.results,
			overall,
			payload: streamed.payload,
			pg_version: None,
		},
		interrupted,
	})
}

struct StreamedSweep {
	payload: Value,
	server_id: Option<String>,
	results: Vec<(Check, bool)>,
}

async fn drain_recompute_stream(
	response: reqwest::Response,
	progress: Option<ProgressSender>,
) -> Result<StreamedSweep> {
	use bestool_alertd::doctor::progress::DoctorEvent;
	use futures::StreamExt as _;

	let registry = checks::all();
	let resolve_name = |s: &str| registry.iter().find(|e| e.name == s).map(|e| e.name);

	let mut stream = response.bytes_stream();
	let mut buffer = Vec::<u8>::new();
	let mut final_payload: Option<Value> = None;
	let mut server_id: Option<String> = None;
	let mut results: Vec<(Check, bool)> = Vec::new();

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
					{
						if let Some(tx) = progress.as_ref() {
							let _ = tx.send(DoctorEvent::Completed(check.clone()));
						}
						results.push((check, true));
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

	let payload = final_payload
		.ok_or_else(|| miette!("alertd recompute stream ended without a done event"))?;
	Ok(StreamedSweep {
		payload,
		server_id,
		results,
	})
}

/// Read the alertd daemon's most recent sweep over `/tasks/doctor/latest`.
async fn fetch_daemon_latest(http: &reqwest::Client) -> Result<(SweepResult, jiff::Timestamp)> {
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
	let results = results_from_wire(&inner);
	Ok((
		SweepResult {
			server_id,
			results,
			overall,
			payload: inner,
			pg_version: None,
		},
		computed_at,
	))
}

/// Reconstruct per-check entries from the daemon's wire payload so the cached
/// path can render the check list and accurate result-line counts. The wire
/// format drops summaries and reasons, so reconstructed entries have empty
/// strings for those fields.
fn results_from_wire(payload: &Value) -> Vec<(Check, bool)> {
	let Some(health) = payload.get("health").and_then(Value::as_array) else {
		return Vec::new();
	};
	let registry = checks::all();
	health
		.iter()
		.filter_map(|entry| {
			let name = entry.get("check").and_then(Value::as_str)?;
			let result = entry.get("result").and_then(Value::as_str)?;
			let name_static = registry.iter().find(|e| e.name == name)?.name;
			let status = match result {
				"passed" => CheckStatus::Pass,
				"skipped" => CheckStatus::Skip(String::new()),
				"warning" => CheckStatus::Warning(String::new()),
				"failed" => CheckStatus::Fail(String::new()),
				"broken" => CheckStatus::Broken(String::new()),
				_ => return None,
			};
			Some((
				Check {
					name: name_static,
					status,
					summary: String::new(),
					details: serde_json::Map::new(),
					payload_extras: serde_json::Map::new(),
				},
				true,
			))
		})
		.collect()
}

/// Set up the progress channel and (when running in a TTY) the live TUI task.
/// The TUI task ends either when every selected check has reported a result or
/// when the user interrupts. When `live_tty` is false (non-interactive output
/// or JSON), no TUI is spawned and the sweep simply runs silently.
fn setup_progress(
	live_tty: bool,
	selected_names: &[&'static str],
	source: SweepSource,
) -> (
	Option<ProgressSender>,
	Option<tokio::task::JoinHandle<Result<tui::TuiOutcome>>>,
) {
	if !live_tty {
		return (None, None);
	}
	let (tx, rx) = mpsc::unbounded_channel();
	let names = selected_names.to_vec();
	let handle = tokio::spawn(tui::run_tui(names, source, rx));
	(Some(tx), Some(handle))
}

fn synthetic_sweep(results: Vec<(Check, bool)>) -> SweepResult {
	let overall =
		OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
	SweepResult {
		server_id: None,
		results,
		overall,
		payload: Value::Object(Default::default()),
		pg_version: None,
	}
}

fn selected_names(only: &[String], skip: &[String]) -> Result<Vec<&'static str>> {
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

fn emit_output(
	args: &DoctorArgs,
	sweep: &SweepResult,
	source: &SweepSource,
	use_colours: bool,
) -> Result<()> {
	let stdout = std::io::stdout();
	let mut out = stdout.lock();

	if args.json {
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

	let sorted = order::filter_and_sort(&sweep.results, true);
	render::render_plain(&mut out, &sorted, args.all, sweep.overall, source, use_colours)
		.into_diagnostic()?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn selected_names_default_returns_full_registry() {
		let names = selected_names(&[], &[]).unwrap();
		let registry: Vec<&str> = checks::all().iter().map(|e| e.name).collect();
		assert_eq!(names, registry);
	}

	#[test]
	fn selected_names_only_filters_to_listed() {
		let names = selected_names(&["db_connect".into(), "memory".into()], &[]).unwrap();
		assert_eq!(names, vec!["db_connect", "memory"]);
	}

	#[test]
	fn selected_names_skip_excludes_listed() {
		let names = selected_names(&[], &["tailscale".into()]).unwrap();
		assert!(!names.contains(&"tailscale"));
		assert!(names.contains(&"db_connect"));
	}

	#[test]
	fn selected_names_only_and_skip_compose() {
		let names = selected_names(
			&["db_connect".into(), "memory".into(), "tailscale".into()],
			&["tailscale".into()],
		)
		.unwrap();
		assert_eq!(names, vec!["db_connect", "memory"]);
	}

	#[test]
	fn selected_names_unknown_skip_is_error() {
		let err = selected_names(&[], &["does_not_exist".into()]).unwrap_err();
		assert!(format!("{err}").contains("does_not_exist"));
	}

	#[test]
	fn synthetic_sweep_marks_overall_from_results() {
		let results = vec![(Check::fail("a", "bad", "r"), true)];
		let sweep = synthetic_sweep(results);
		assert_eq!(sweep.overall, OverallResult::Failing);
	}

	#[test]
	fn doctor_args_all_short_flag() {
		use clap::Parser;
		let parsed = DoctorArgs::parse_from(["doctor", "-a"]);
		assert!(parsed.all);
	}

	#[test]
	fn doctor_args_all_long_flag() {
		use clap::Parser;
		let parsed = DoctorArgs::parse_from(["doctor", "--all"]);
		assert!(parsed.all);
	}

	#[test]
	fn doctor_args_default_filters_replay() {
		use clap::Parser;
		let parsed = DoctorArgs::parse_from(["doctor"]);
		assert!(!parsed.all);
	}

	#[test]
	fn results_from_wire_reconstructs_per_check_entries() {
		let registry = checks::all();
		let known = registry[0].name;
		let payload = serde_json::json!({
			"health": [
				{ "check": known, "result": "passed" },
				{ "check": "unknown_check_name", "result": "failed" },
			]
		});
		let results = results_from_wire(&payload);
		assert_eq!(results.len(), 1);
		assert_eq!(results[0].0.name, known);
		assert!(matches!(results[0].0.status, CheckStatus::Pass));
	}

	#[test]
	fn results_from_wire_empty_when_no_health_array() {
		let payload = serde_json::json!({});
		assert!(results_from_wire(&payload).is_empty());
	}
}
