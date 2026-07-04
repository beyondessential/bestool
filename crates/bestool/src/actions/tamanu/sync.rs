use std::{process::Stdio, str::FromStr, time::Duration};

use clap::Parser;
use jiff::{SignedDuration, Timestamp};
use miette::{IntoDiagnostic, Result, bail};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use bestool_tamanu::{ApiServerKind, config::load_config, services::Supervisor};

use crate::actions::{
	Context,
	tamanu::{TamanuArgs, find_tamanu},
};

/// Trigger a manual sync on a facility server and watch it run.
///
/// Sends `POST /sync/run` to the local facility sync sub-process
/// (`http://localhost:4100` by default, bound to localhost and not
/// authed). If central queues the device (`{ ran: false, queued: true }`),
/// retries until central lets the sync run or `--start-timeout` elapses.
/// Once a sync runs, cross-checks `GET /sync/status` to confirm
/// `lastCompletedAt` actually advanced — so a `ran: true` response that
/// was somehow stale won't be silently accepted.
///
/// While the sync runs, the command tails the sync service's logs so
/// the operator can see what's happening.
///
/// Only valid on facility servers — central servers have no sync
/// sub-process to talk to.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct SyncArgs {
	/// Number of trailing log lines to print before tailing.
	#[arg(short = 'n', long = "lines", default_value = "10")]
	pub lines: usize,

	/// Just trigger the sync, don't tail the service logs.
	#[arg(long)]
	pub no_follow: bool,

	/// How long to wait between retries when central has queued the
	/// device. Matches the cadence Tamanu's own facility-server CLI
	/// uses (15s).
	#[arg(long, value_parser = parse_duration, default_value = "15s")]
	pub retry_interval: Duration,

	/// Exit non-zero if central is still queueing the device (the sync
	/// hasn't *started*) after this long. Default: no limit; keep
	/// retrying.
	#[arg(long, value_parser = parse_duration)]
	pub start_timeout: Option<Duration>,

	/// Exit non-zero if the sync hasn't *completed* (including all
	/// retries) within this long. Default: no limit.
	#[arg(long, value_parser = parse_duration)]
	pub timeout: Option<Duration>,
}

fn parse_duration(s: &str) -> Result<Duration, String> {
	s.parse::<SignedDuration>()
		.map_err(|e| e.to_string())
		.and_then(|d| Duration::try_from(d).map_err(|e| e.to_string()))
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct SyncRunResult {
	enabled: bool,
	ran: bool,
	queued: bool,
}

impl Default for SyncRunResult {
	fn default() -> Self {
		Self {
			enabled: true,
			ran: false,
			queued: false,
		}
	}
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct SyncStatus {
	is_sync_running: bool,
	last_completed_at: Value,
	last_completed_duration_ms: Option<u64>,
}

/// `lastCompletedAt` is serialised as the JS `Date` object's JSON form
/// (ISO 8601 string) once a sync has run, but stays as the literal
/// number `0` on a fresh install (the JS field is initialised to `0`
/// and only reassigned to `new Date()` after the first successful
/// sync). Normalise both forms to epoch milliseconds; `0` means "never
/// completed", which is a valid baseline on a fresh facility.
fn completed_at_to_ms(v: &Value) -> i64 {
	match v {
		Value::Number(n) => n.as_i64().unwrap_or(0),
		Value::String(s) => Timestamp::from_str(s).map(|t| t.as_millisecond()).unwrap_or(0),
		_ => 0,
	}
}

pub async fn run(args: SyncArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();
	let (_, root) = find_tamanu(tamanu).await?;
	let config = load_config(&root, None)?;

	if !config.is_facility() {
		bail!("`tamanu sync` only runs on facility servers (no sync sub-process on central)");
	}

	let kind = ApiServerKind::Facility;
	let supervisor = if cfg!(target_os = "linux") {
		Supervisor::Systemd
	} else if cfg!(target_os = "windows") {
		Supervisor::Pm2
	} else {
		bail!("tamanu sync is only supported on Linux (systemd) and Windows (pm2)");
	};

	let service_name = match supervisor {
		Supervisor::Pm2 => "tamanu-sync",
		Supervisor::Systemd => "tamanu-facility-sync",
	};

	let base_url = config
		.sync_api_url()
		.ok_or_else(|| miette::miette!("config has no sync.syncApiConnection block"))?;
	debug!(%base_url, %service_name, ?kind, "preparing sync");

	let client = crate::http::client_builder()
		// No client-side timeout: we manage the overall and start timeouts
		// ourselves, around the whole retry loop. The sync sub-process's
		// `/sync/run` has no server-side timeout either, so a single POST can
		// legitimately take minutes.
		.build()
		.into_diagnostic()?;

	let baseline = fetch_status(&client, &base_url)
		.await?
		.map(|s| completed_at_to_ms(&s.last_completed_at))
		.unwrap_or(0);
	debug!(baseline, "baseline lastCompletedAt");

	// Spawn log tailing for the whole operation. `kill_on_drop(true)` means
	// the child journalctl/pm2 process is reaped when this binding goes out
	// of scope, including the bail!() paths below.
	let _log_child = if args.no_follow {
		None
	} else {
		Some(spawn_log_child(supervisor, service_name, args.lines)?)
	};

	sync_loop(&client, &base_url, &args, baseline).await
}

async fn sync_loop(
	client: &Client,
	base_url: &Url,
	args: &SyncArgs,
	baseline: i64,
) -> Result<()> {
	let mut run_url = base_url.clone();
	run_url.set_path("/sync/run");

	let body = serde_json::json!({
		"syncData": { "type": "bestool", "urgent": true },
	});

	let started = Instant::now();
	let mut attempt: u32 = 0;

	loop {
		attempt += 1;

		// Overall timeout: race the POST against the remaining overall budget,
		// so even a single very long-running request honours `--timeout`.
		let remaining_overall = match args.timeout {
			Some(t) => match t.checked_sub(started.elapsed()) {
				Some(r) => Some(r),
				None => bail!("--timeout {t:?} reached before sync completed"),
			},
			None => None,
		};

		info!(attempt, %run_url, "POST /sync/run");
		let request = client.post(run_url.clone()).json(&body).send();
		let response = match remaining_overall {
			Some(r) => match tokio::time::timeout(r, request).await {
				Ok(resp) => resp.into_diagnostic()?,
				Err(_) => bail!(
					"--timeout {:?} reached while POST /sync/run was in flight",
					args.timeout.unwrap()
				),
			},
			None => request.await.into_diagnostic()?,
		};

		let status = response.status();
		let text = response.text().await.into_diagnostic()?;
		debug!(?status, %text, "sync run response");

		if !status.is_success() {
			bail!("sync trigger returned HTTP {status}: {text}");
		}

		let result: SyncRunResult = serde_json::from_str(&text).map_err(|e| {
			miette::miette!("could not parse sync response (HTTP {status}): {e}\nbody: {text}")
		})?;

		match (result.enabled, result.ran, result.queued) {
			(false, _, _) => bail!("sync is disabled in config (sync.enabled = false)"),
			(true, true, _) => {
				return confirm_completed(client, base_url, baseline).await;
			}
			(true, false, true) => {
				let elapsed = started.elapsed();
				info!(
					attempt,
					elapsed_s = elapsed.as_secs(),
					retry_in_s = args.retry_interval.as_secs(),
					"central queued the device — sync hasn't started yet, will retry"
				);

				if let Some(t) = args.start_timeout
					&& elapsed >= t
				{
					bail!(
						"--start-timeout {t:?} reached: central is still queueing the device after {attempt} attempt(s) over {elapsed:?}"
					);
				}

				tokio::time::sleep(args.retry_interval).await;
				continue;
			}
			(true, false, false) => {
				bail!("sync neither ran nor queued — unknown state: {text}")
			}
		}
	}
}

/// Cross-check `/sync/status.lastCompletedAt` after a `ran: true`
/// response. If it didn't advance past the baseline we captured before
/// firing the trigger, the response is at odds with the sub-process's
/// own view — treat that as a hard failure rather than silently
/// trusting it, since the whole point of this command is to *confirm*
/// a sync ran end-to-end.
async fn confirm_completed(client: &Client, base_url: &Url, baseline: i64) -> Result<()> {
	let status = fetch_status(client, base_url).await?.ok_or_else(|| {
		miette::miette!("/sync/run returned ran=true but /sync/status returned nothing to confirm it")
	})?;
	let current = completed_at_to_ms(&status.last_completed_at);
	if current <= baseline {
		bail!(
			"sync sub-process returned ran=true but lastCompletedAt did not advance (baseline={baseline}, current={current})"
		);
	}
	info!(
		duration_ms = status.last_completed_duration_ms,
		last_completed_at = ?status.last_completed_at,
		"sync completed"
	);
	Ok(())
}

async fn fetch_status(client: &Client, base_url: &Url) -> Result<Option<SyncStatus>> {
	let mut url = base_url.clone();
	url.set_path("/sync/status");
	let resp = match client.get(url.clone()).send().await {
		Ok(r) => r,
		Err(e) => {
			warn!(err = %e, "could not query /sync/status");
			return Ok(None);
		}
	};
	if !resp.status().is_success() {
		warn!(status = %resp.status(), "non-success from /sync/status");
		return Ok(None);
	}
	let text = resp.text().await.into_diagnostic()?;
	let status: SyncStatus = serde_json::from_str(&text)
		.map_err(|e| miette::miette!("could not parse /sync/status response: {e}\nbody: {text}"))?;
	Ok(Some(status))
}

fn spawn_log_child(
	supervisor: Supervisor,
	service: &str,
	lines: usize,
) -> Result<tokio::process::Child> {
	let mut cmd = match supervisor {
		Supervisor::Systemd => {
			let mut c = Command::new("journalctl");
			c.arg("-u")
				.arg(format!("{service}.service"))
				.arg("-n")
				.arg(lines.to_string())
				.arg("-f")
				.arg("--output=cat");
			c
		}
		Supervisor::Pm2 => {
			let (program, prefix_args) = bestool_tamanu::pm2::invocation();
			let mut c = Command::new(program);
			c.args(prefix_args)
				.arg("logs")
				.arg(service)
				.arg("--lines")
				.arg(lines.to_string());
			c
		}
	};
	cmd.stdout(Stdio::piped())
		.stderr(Stdio::inherit())
		.kill_on_drop(true);

	let mut child = cmd.spawn().into_diagnostic()?;
	let stdout = child
		.stdout
		.take()
		.ok_or_else(|| miette::miette!("log child had no stdout pipe"))?;

	// Pipe the log child's stdout to ours line-by-line. The task exits when
	// the child is killed (on completion) or when we hit EOF.
	tokio::spawn(async move {
		let reader = BufReader::new(stdout);
		let mut lines = reader.lines();
		while let Ok(Some(line)) = lines.next_line().await {
			if line.trim().is_empty() {
				// journalctl --output=cat emits blank lines for some
				// records; same treatment as `tamanu logs`.
				continue;
			}
			println!("{line}");
		}
	});

	Ok(child)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn completed_at_zero_number_means_never() {
		assert_eq!(completed_at_to_ms(&serde_json::json!(0)), 0);
	}

	#[test]
	fn completed_at_iso_string_parses_to_epoch_ms() {
		let v = serde_json::json!("2026-05-27T10:00:00.000Z");
		let ms = completed_at_to_ms(&v);
		assert!(ms > 0);
		// Sanity: a much later timestamp should be strictly greater.
		let later = serde_json::json!("2026-05-27T11:00:00.000Z");
		assert!(completed_at_to_ms(&later) > ms);
	}

	#[test]
	fn completed_at_unrecognised_shape_is_zero() {
		assert_eq!(completed_at_to_ms(&serde_json::json!(null)), 0);
		assert_eq!(completed_at_to_ms(&serde_json::json!("not a date")), 0);
		assert_eq!(completed_at_to_ms(&serde_json::json!({})), 0);
	}
}
