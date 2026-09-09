use std::{
	collections::HashMap,
	io::{IsTerminal as _, Write},
	time::Duration,
};

use bestool_canopy::schema::{CheckResult, CheckSeverity, HealthCheck, StatusPayload};
use clap::Parser;
use miette::{IntoDiagnostic, Result, WrapErr, miette};
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use bestool_alertd::doctor::{
	SweepResult, SweepTamanu,
	check::{Check, CheckOutcome, CheckStatus, OverallResult},
	checks, overall_from_payload, perform_sweep,
	progress::ProgressSender,
	resolve_sweep_tamanu,
	subject::{ApplicationKind, ApplicationRef, Subject},
	sweep::{SplitSeverities, validate_selection},
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

pub async fn run(args: DoctorArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();

	// When ANSI escapes are not honoured (e.g. an older Windows console where
	// virtual terminal processing cannot be enabled), the live TUI cannot render
	// and styled output would print as garbage, so fall back to plain
	// non-interactive output.
	let ansi = ansi_supported();
	let use_colours = tamanu.use_colours && ansi;

	let install = resolve_sweep_tamanu(try_find_tamanu(tamanu).await?)?;
	if install.is_none() {
		warn!("no Tamanu on this host; running host-level checks only");
	}
	let http_client = crate::http::client();

	let live_tty = !args.json && ansi && std::io::stdout().is_terminal();

	let (sweep, source, interrupted) = if args.no_daemon {
		let outcome = run_local_sweep(install.clone(), http_client.clone(), &args, live_tty).await?;
		(outcome.sweep, SweepSource::Local, outcome.interrupted)
	} else if args.fresh {
		match run_daemon_recompute(&http_client, &args, live_tty).await {
			Ok(outcome) => (outcome.sweep, SweepSource::DaemonStreamed, outcome.interrupted),
			Err(err) => {
				warn!(%err, "alertd did not answer; ran the checks locally instead of on the daemon");
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
	validate_check_selection(&args.only, &args.skip)?;
	let (progress, tui_handle) = setup_progress(live_tty, SweepSource::Local);

	// Fetch canopy's effective-severity ceilings concurrently with the checks.
	// Soft-fail throughout (see `fetch_check_severities`): the mapping only ever
	// lowers a verdict, so its absence just leaves the raw sweep.
	let severities_handle = tokio::spawn(fetch_check_severities());

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
			None,
			false,
		)
		.await
	});

	let interrupted = if let Some(handle) = tui_handle {
		let outcome = handle.await.into_diagnostic()??;
		if outcome.interrupted {
			sweep_handle.abort();
			severities_handle.abort();
			let mut synthetic = synthetic_sweep(outcome.results);
			synthetic.payload = empty_payload();
			return Ok(SweepOutcome {
				sweep: synthetic,
				interrupted: true,
			});
		}
		false
	} else {
		false
	};

	let mut sweep = sweep_handle.await.into_diagnostic()??;
	// Apply the mapping once it's back (the checks may well have finished first).
	if let Some(severities) = severities_handle.await.ok().flatten() {
		sweep.apply_severities(&SplitSeverities::flat(severities));
	}
	Ok(SweepOutcome { sweep, interrupted })
}

/// Best-effort fetch of canopy's effective-severity ceilings for this server.
///
/// Every failure path — no registration, no auth path to canopy, no resolvable
/// server id, a timeout, or any request/parse error — resolves to `None` ("no
/// mapping") rather than an error, so a doctor run never fails or stalls on
/// canopy being unreachable. Bounded by an overall timeout for the same reason.
async fn fetch_check_severities() -> Option<HashMap<String, CheckSeverity>> {
	tokio::time::timeout(Duration::from_secs(10), async {
		let reg = bestool_canopy::registration::load().await.ok().flatten()?;
		let device_key = reg.device_key.as_deref()?;
		let base_url = reg
			.api_url
			.as_deref()
			.unwrap_or(bestool_canopy::DEFAULT_CANOPY_URL)
			.parse()
			.ok()?;
		let tailscale_url = bestool_canopy::TAILSCALE_URL.parse().ok()?;
		let client = bestool_canopy::connect_to(
			base_url,
			tailscale_url,
			Some(device_key),
			crate::http::client_builder,
		)
		.await
		.ok()??;

		let machine_id = bestool_tamanu::server_info::get_or_create_machine_id()
			.await
			.ok()?;
		client.status_check_severities(&machine_id).await.ok()
	})
	.await
	.ok()
	.flatten()
}

/// Drive a fresh sweep on the daemon and stream the per-check results back.
async fn run_daemon_recompute(
	http: &reqwest::Client,
	args: &DoctorArgs,
	live_tty: bool,
) -> Result<SweepOutcome> {
	let response = crate::http::daemon_get(http, "/tasks/doctor/recompute", |r| {
		r.timeout(std::time::Duration::from_secs(5))
	})
	.await
	.into_diagnostic()
	.wrap_err("contacting local alertd")?;

	if !response.status().is_success() {
		return Err(miette!(
			"alertd /tasks/doctor/recompute returned {}",
			response.status()
		));
	}

	validate_check_selection(&args.only, &args.skip)?;
	let (progress, tui_handle) = setup_progress(live_tty, SweepSource::DaemonStreamed);

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
			machine_id: streamed.machine_id,
			results: streamed.results,
			overall,
			payload: streamed.payload,
			pg_version: None,
		},
		interrupted,
	})
}

struct StreamedSweep {
	payload: StatusPayload,
	machine_id: Option<String>,
	results: Vec<CheckOutcome>,
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
	let mut final_payload: Option<StatusPayload> = None;
	let mut machine_id: Option<String> = None;
	let mut results: Vec<CheckOutcome> = Vec::new();

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
				// The daemon resolved the applications; pass its plan through so the
				// live display shows the same checks pending from the start.
				Some("planned") => {
					if let Some(tx) = progress.as_ref()
						&& let Some(checks) = value.get("checks").and_then(Value::as_array)
					{
						let names = checks
							.iter()
							.filter_map(Value::as_str)
							.map(str::to_string)
							.collect();
						let _ = tx.send(DoctorEvent::Planned(names));
					}
				}
				Some("check") => {
					if let Some(check_json) = value.get("check")
						&& let Some(outcome) =
							CheckOutcome::from_streaming_json(check_json, resolve_name)
					{
						if let Some(tx) = progress.as_ref() {
							let _ = tx.send(DoctorEvent::Completed(outcome.clone()));
						}
						results.push(outcome);
					}
				}
				Some("done") => {
					// A payload that will not decode is a real failure, not a missing
					// done event, and must say which it is.
					let raw = value
						.get("payload")
						.cloned()
						.ok_or_else(|| miette!("alertd recompute done event carried no payload"))?;
					final_payload = Some(
						serde_json::from_value(raw)
							.into_diagnostic()
							.wrap_err("decoding the alertd recompute payload")?,
					);
					machine_id = value
						.get("machineId")
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
		machine_id,
		results,
	})
}

/// Read the alertd daemon's most recent sweep over `/tasks/doctor/latest`.
async fn fetch_daemon_latest(http: &reqwest::Client) -> Result<(SweepResult, jiff::Timestamp)> {
	let response = crate::http::daemon_get(http, "/tasks/doctor/latest", |r| {
		r.timeout(std::time::Duration::from_secs(3))
	})
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

	let inner: StatusPayload = payload
		.get("payload")
		.cloned()
		.ok_or_else(|| miette!("alertd latest payload missing payload"))
		.and_then(|p| {
			serde_json::from_value(p)
				.into_diagnostic()
				.wrap_err("decoding alertd latest status payload")
		})?;
	let machine_id = payload
		.get("machineId")
		.and_then(Value::as_str)
		.map(str::to_string);

	let overall = overall_from_payload(&inner);
	let results = results_from_wire(&inner);
	Ok((
		SweepResult {
			machine_id,
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
///
/// Every grain is read: the machine's checks, each application's, and the
/// ungrouped array, each tagged with the subject it was filed against so the
/// render can tell two same-named checks apart.
fn results_from_wire(payload: &StatusPayload) -> Vec<CheckOutcome> {
	let registry = checks::all();
	let names: HashMap<&str, &'static str> = registry.iter().map(|e| (e.name, e.name)).collect();

	let mut targets: Vec<(Subject, &Vec<HealthCheck>)> = Vec::new();

	// A daemon that predates the split describes no targets and puts every check
	// in the ungrouped array. Those are read against the machine: which subject
	// each belonged to is exactly what that daemon did not say, and the machine
	// is the grain an operator reads such a sweep at. A sweep of this vintage
	// sends the array empty, so this contributes nothing to one.
	if !payload.health.is_empty() {
		targets.push((Subject::Machine, &payload.health));
	}
	if let Some(machine) = payload.machine.as_ref()
		&& let Some(health) = machine.health.as_ref()
	{
		targets.push((Subject::Machine, health));
	}
	for (key, report) in payload.applications.iter().flatten() {
		// The type slug the push used is the application's own, so the kind is
		// read back from it. The wire type is an open set, so a target of a type
		// this build does not know — an mSupply application, or a newer one from
		// a mismatched daemon — is left out rather than rendered as some other
		// kind's.
		let Some(kind) = ApplicationKind::ALL
			.into_iter()
			.find(|kind| kind.type_slug() == report.type_)
		else {
			continue;
		};
		if let Some(health) = report.health.as_ref() {
			let app = ApplicationRef {
				kind,
				key: key.clone(),
			};
			targets.push((Subject::Application(app), health));
		}
	}

	let mut results = Vec::new();
	for (subject, health) in targets {
		for entry in health {
			let Some(name) = names.get(entry.check.as_str()).copied() else {
				continue;
			};
			let Some(result) = entry.result.as_ref() else {
				continue;
			};
			let status = match result {
				CheckResult::Passed => CheckStatus::Pass,
				CheckResult::Skipped => CheckStatus::Skip(String::new()),
				CheckResult::Warning => CheckStatus::Warning(String::new()),
				CheckResult::Failed => CheckStatus::Fail(String::new()),
				CheckResult::Broken => CheckStatus::Broken(String::new()),
			};
			results.push(CheckOutcome {
				subject: subject.clone(),
				check: Check {
					name,
					status,
					summary: String::new(),
					details: serde_json::Map::new(),
					payload_extras: serde_json::Map::new(),
					stats: Vec::new(),
				},
				on_wire: true,
			});
		}
	}
	results
}

/// Whether the terminal honours ANSI escape sequences, which the live TUI and
/// styled output both rely on. On Windows this attempts to enable virtual
/// terminal processing; if that fails on an older console, crossterm falls back
/// to a winapi screen-buffer path that our buffered drawing cannot target, so
/// we treat ANSI as unavailable rather than render a black screen.
fn ansi_supported() -> bool {
	#[cfg(windows)]
	{
		crossterm::ansi_support::supports_ansi()
	}
	#[cfg(not(windows))]
	{
		true
	}
}

/// Set up the progress channel and (when running in a TTY) the live TUI task.
/// The TUI task ends either when every selected check has reported a result or
/// when the user interrupts. When `live_tty` is false (non-interactive output
/// or JSON), no TUI is spawned and the sweep simply runs silently.
fn setup_progress(
	live_tty: bool,
	source: SweepSource,
) -> (
	Option<ProgressSender>,
	Option<tokio::task::JoinHandle<Result<tui::TuiOutcome>>>,
) {
	if !live_tty {
		return (None, None);
	}
	let (tx, rx) = mpsc::unbounded_channel();
	let handle = tokio::task::spawn_blocking(move || tui::run_tui(source, rx));
	(Some(tx), Some(handle))
}

fn synthetic_sweep(results: Vec<CheckOutcome>) -> SweepResult {
	let overall =
		OverallResult::from_checks(&results.iter().map(|o| o.check.clone()).collect::<Vec<_>>());
	SweepResult {
		machine_id: None,
		results,
		overall,
		payload: empty_payload(),
		pg_version: None,
	}
}

/// A payload describing no subjects, for the paths that carry results without
/// ever having built a push: an interrupted sweep, and the TUI's own result
/// collection.
fn empty_payload() -> StatusPayload {
	StatusPayload::builder().health(Vec::new()).build()
}

/// Reject a bad `--check` or `--skip` before the terminal is taken over, using
/// the same validation the sweep applies.
///
/// One contract, so a name the sweep would run is never rejected here first, and
/// a bare name is refused identically by both. Which checks actually run is the
/// sweep's to decide and announce, since only it knows what applications the
/// host has.
///
/// spec: DOC
fn validate_check_selection(only: &[String], skip: &[String]) -> Result<()> {
	let registry = checks::all();
	validate_selection(&registry, only, "--check")?;
	validate_selection(&registry, skip, "--skip")
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
		wrapped.insert(
			"wire".into(),
			serde_json::to_value(&sweep.payload).into_diagnostic()?,
		);
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
	fn no_selection_flags_is_accepted() {
		assert!(validate_check_selection(&[], &[]).is_ok());
	}

	#[test]
	fn the_cli_accepts_a_qualified_name_the_sweep_accepts() {
		// One contract: a name the sweep would run must not be rejected here
		// first, and a bare name must be rejected the same way in both.
		assert!(validate_check_selection(&["postgres:connect".into()], &[]).is_ok());
		assert!(validate_check_selection(&["machine:memory".into()], &[]).is_ok());
		let err = validate_check_selection(&["connect".into()], &[]).unwrap_err();
		assert!(format!("{err}").contains("postgres:connect"));
	}

	#[test]
	fn skip_rejects_a_bare_name_on_the_same_terms_as_check() {
		let err = validate_check_selection(&[], &["memory".into()]).unwrap_err();
		let msg = format!("{err}");
		assert!(msg.contains("--skip"), "{msg}");
		assert!(msg.contains("machine:memory"), "{msg}");
	}

	#[test]
	fn unknown_skip_is_error() {
		let err = validate_check_selection(&[], &["does_not_exist".into()]).unwrap_err();
		assert!(format!("{err}").contains("does_not_exist"));
	}

	#[test]
	fn synthetic_sweep_marks_overall_from_results() {
		let results = vec![CheckOutcome {
			subject: Subject::Machine,
			check: Check::fail("a", "bad", "r"),
			on_wire: true,
		}];
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
		let payload: StatusPayload = serde_json::from_value(serde_json::json!({
			"health": [],
			"machine": {
				"detail": {},
				"health": [
					{ "check": "disk_free", "result": "passed" },
					{ "check": "unknown_check_name", "result": "failed" },
				],
			},
		}))
		.unwrap();
		let results = results_from_wire(&payload);
		assert_eq!(results.len(), 1);
		assert_eq!(results[0].check.name, "disk_free");
		assert_eq!(results[0].subject, Subject::Machine);
		assert!(matches!(results[0].check.status, CheckStatus::Pass));
	}

	#[test]
	fn results_from_wire_reads_both_grains() {
		// The cached path renders the machine's checks and the application's, and
		// must keep them apart even when a name appears under both.
		let payload: StatusPayload = serde_json::from_value(serde_json::json!({
			"health": [],
			"machine": {
				"detail": {},
				"health": [{ "check": "disk_free", "result": "passed" }],
			},
			"applications": {
				"host-tamanu-central": {
					"type": "tamanu-central",
					"detail": {},
					"health": [{ "check": "migrations", "result": "failed" }],
				},
			},
		}))
		.unwrap();
		let results = results_from_wire(&payload);
		assert_eq!(results.len(), 2);

		let subject_of = |name: &str| {
			results
				.iter()
				.find(|o| o.check.name == name)
				.map(|o| o.subject.clone())
				.unwrap()
		};
		assert_eq!(subject_of("disk_free"), Subject::Machine);
		assert_eq!(
			subject_of("migrations"),
			Subject::Application(ApplicationRef::tamanu(ApplicationKind::TamanuCentral))
		);
	}

	#[test]
	fn a_pre_split_payload_is_read_rather_than_called_healthy() {
		// A daemon that predates the split describes no targets and puts every
		// check in the ungrouped array. Reading only the targets would render
		// such a sweep healthy however much of it had failed.
		let payload: StatusPayload = serde_json::from_value(serde_json::json!({
			"health": [
				{ "check": "disk_free", "result": "failed" },
				{ "check": "memory", "result": "passed" },
			],
		}))
		.unwrap();

		assert_eq!(
			bestool_alertd::doctor::overall_from_payload(&payload),
			OverallResult::Failing,
		);
		let results = results_from_wire(&payload);
		assert_eq!(results.len(), 2);
		assert!(results.iter().all(|o| o.subject == Subject::Machine));
	}

	#[test]
	fn results_from_wire_empty_when_no_targets() {
		let payload = empty_payload();
		assert!(results_from_wire(&payload).is_empty());
	}
}
