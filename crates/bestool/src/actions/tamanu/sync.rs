use std::{process::Stdio, time::Duration};

use clap::Parser;
use jiff::SignedDuration;
use miette::{IntoDiagnostic, Result, bail};
use reqwest::Client;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
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
/// authed). The request blocks until the sync completes; while it
/// runs, this command tails the sync service's logs so the operator
/// can see what's happening.
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

	/// Give up if the sync hasn't responded after this long. By default
	/// the command waits indefinitely — `/sync/run` itself has no
	/// server-side timeout and a real sync can take minutes against a
	/// busy central.
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

pub async fn run(args: SyncArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();
	let (_, root) = find_tamanu(tamanu)?;
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

	let mut sync_url = config
		.sync_api_url()
		.ok_or_else(|| miette::miette!("config has no sync.syncApiConnection block"))?;
	sync_url.set_path("/sync/run");
	debug!(%sync_url, %service_name, ?kind, "triggering sync");

	let mut log_child = if args.no_follow {
		None
	} else {
		Some(spawn_log_child(supervisor, service_name, args.lines)?)
	};

	let client = Client::builder()
		// No timeout: a real sync can take minutes against a busy central. The
		// sync sub-process's `/sync/run` has no server-side timeout either.
		.build()
		.into_diagnostic()?;
	let body = serde_json::json!({
		"syncData": { "type": "bestool", "urgent": true },
	});
	info!(%sync_url, "POST /sync/run");
	let request = client.post(sync_url.clone()).json(&body).send();
	let response = match args.timeout {
		Some(timeout) => match tokio::time::timeout(timeout, request).await {
			Ok(resp) => resp.into_diagnostic()?,
			Err(_) => {
				// Drop the log child before bailing so the operator's terminal
				// isn't left with a backgrounded journalctl/pm2 process.
				if let Some(mut child) = log_child.take() {
					let _ = child.start_kill();
				}
				bail!("sync trigger did not respond within {timeout:?}");
			}
		},
		None => request.await.into_diagnostic()?,
	};

	let status = response.status();
	let text = response.text().await.into_diagnostic()?;
	debug!(?status, %text, "sync run response");

	// Stop tailing now that the sync has resolved.
	if let Some(mut child) = log_child.take() {
		let _ = child.start_kill();
	}

	if !status.is_success() {
		bail!("sync trigger returned HTTP {status}: {text}");
	}

	let result: SyncRunResult = serde_json::from_str(&text).map_err(|e| {
		miette::miette!("could not parse sync response (HTTP {status}): {e}\nbody: {text}")
	})?;

	match (result.enabled, result.ran, result.queued) {
		(false, _, _) => bail!("sync is disabled in config (sync.enabled = false)"),
		(true, true, _) => {
			info!("sync completed");
			Ok(())
		}
		(true, false, true) => {
			warn!("sync was queued — central is busy, retry later");
			Ok(())
		}
		(true, false, false) => {
			bail!("sync neither ran nor queued — unknown state: {text}")
		}
	}
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
			let mut c = Command::new("pm2");
			c.arg("logs").arg(service).arg("--lines").arg(lines.to_string());
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
	// the child is killed (on POST completion) or when we hit EOF.
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
