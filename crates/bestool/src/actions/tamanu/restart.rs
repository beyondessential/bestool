use std::time::Duration;

use clap::Parser;
use jiff::SignedDuration;
use miette::{IntoDiagnostic, Result, bail};
use reqwest::{Client, Url};
use tracing::{debug, info, warn};

use bestool_tamanu::services::{self, Criticality, ExpectedState, Expectation, Supervisor};

use crate::actions::{
	Context,
	tamanu::{
		TamanuArgs,
		lifecycle::{self, Instance},
	},
};

/// Rolling-restart all running tamanu services.
///
/// Background services (tasks, sync, fhir-*) restart in a single bulk
/// supervisor call. Critical services (api, frontend) restart one
/// instance at a time, each followed by a readiness probe, caddy
/// reload, and a cooldown — so there's always at least one critical
/// instance up to take traffic.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct RestartArgs {
	/// Limit to expectations whose name contains any of these substrings.
	/// No names = restart every running instance of every Up expectation.
	pub names: Vec<String>,

	/// Sleep between each critical-instance roll when the HTTP probe is
	/// disabled (`--no-probe-http`). With probes enabled, the readiness
	/// probe is the signal — once a fresh instance responds, we move on
	/// to the next without waiting out the cooldown.
	#[arg(long, default_value = "30s", value_parser = parse_duration)]
	pub cooldown: Duration,

	/// Skip the per-instance HTTP probe. Useful if the deployment isn't
	/// behind caddy (so the netavark IP doesn't matter) or you just want
	/// a fast best-effort restart without waiting on container readiness.
	#[arg(long)]
	pub no_probe_http: bool,

	/// After the rolling restart, hit this URL once to confirm
	/// end-to-end reachability. Bails non-zero if the probe fails.
	#[arg(long, value_name = "URL")]
	pub check_url: Option<Url>,
}

fn parse_duration(s: &str) -> Result<Duration, String> {
	s.parse::<SignedDuration>()
		.map_err(|e| e.to_string())
		.and_then(|d| Duration::try_from(d).map_err(|e| e.to_string()))
}

pub async fn run(args: RestartArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();

	let (supervisor, expectations) = lifecycle::config_and_expectations(tamanu)?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = services::match_names(&expectations, &names)?;
	let discovered = lifecycle::discover(supervisor)?;
	let groups = lifecycle::group_by_expectation(&matched, &discovered);

	lifecycle::ensure_root_or_reexec(supervisor)?;

	let (background, critical) = partition(supervisor, &groups);
	let client = http_client()?;

	if !background.is_empty() {
		info!(targets = ?background, "restarting background services");
		bulk_restart(supervisor, &background)?;
		lifecycle::wait_running(supervisor, &background)?;
	} else {
		debug!("no background services to restart");
	}

	for (i, instance) in critical.iter().enumerate() {
		info!(
			"rolling restart {}/{}: {}",
			i + 1,
			critical.len(),
			instance.display(),
		);
		lifecycle::restart_one(supervisor, instance)?;
		lifecycle::wait_running_one(supervisor, instance, Duration::from_secs(60))?;

		let probed_ready = if !args.no_probe_http {
			// `probe_instance` blocks until the new container responds. We
			// have no reason to give up — the supervisor already says the
			// unit is running, and the container *will* eventually accept
			// connections (or the operator can ctrl+c). When it does, the
			// probe is our readiness signal and we move straight on. The
			// only way this returns `false` is if we couldn't construct a
			// probe URL at all (no container IP, no pm2 port).
			probe_instance(supervisor, instance, &client).await?
		} else {
			false
		};

		if matches!(supervisor, Supervisor::Systemd) {
			lifecycle::reload_caddy();
		}

		// Cooldown only applies when we have no readiness signal at all —
		// either probing is disabled (`--no-probe-http`) or we couldn't
		// construct a probe URL. A failed probe is impossible here: the
		// probe loop retries forever until it succeeds.
		if i + 1 < critical.len() && !probed_ready {
			debug!(seconds = args.cooldown.as_secs(), "cooldown");
			tokio::time::sleep(args.cooldown).await;
		}
	}

	if let Some(url) = &args.check_url {
		info!(%url, "final end-to-end probe");
		probe_url(&client, url, Duration::from_secs(60)).await?;
	}

	Ok(())
}

/// Split discovered instances into (background, critical), keeping only
/// what's currently running. Drops `Down` expectations.
fn partition(
	supervisor: Supervisor,
	groups: &[(&Expectation, Vec<Instance>)],
) -> (Vec<String>, Vec<Instance>) {
	let mut background = Vec::new();
	let mut critical = Vec::new();
	for (exp, instances) in groups {
		if exp.state != ExpectedState::Up {
			continue;
		}
		for inst in instances {
			if !inst.running {
				continue;
			}
			match exp.criticality {
				Criticality::Background => background.push(match supervisor {
					Supervisor::Systemd => inst.unit(),
					Supervisor::Pm2 => inst.name.clone(),
				}),
				Criticality::Critical => critical.push(inst.clone()),
			}
		}
	}
	(background, critical)
}

fn bulk_restart(supervisor: Supervisor, targets: &[String]) -> Result<()> {
	let (cmd, verb) = match supervisor {
		Supervisor::Systemd => ("systemctl", "restart"),
		Supervisor::Pm2 => ("pm2", "restart"),
	};
	let status = std::process::Command::new(cmd)
		.arg(verb)
		.args(targets)
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("{cmd} {verb} failed: {status}");
	}
	Ok(())
}

fn http_client() -> Result<Client> {
	Client::builder()
		.timeout(Duration::from_secs(5))
		.build()
		.into_diagnostic()
}

/// Probe a freshly-restarted instance until it responds.
///
/// Returns `Ok(true)` when the probe loop got a non-5xx response, `Ok(false)`
/// when we couldn't construct a probe URL at all (no container IP, no pm2
/// port). The probe loop itself retries indefinitely — the container we
/// just restarted *will* come up eventually.
async fn probe_instance(
	supervisor: Supervisor,
	instance: &Instance,
	client: &Client,
) -> Result<bool> {
	let url = match supervisor {
		Supervisor::Systemd => {
			let unit = instance.unit();
			match lifecycle::container_ip_for_unit(&unit)? {
				Some(ip) => format!("http://{ip}:3000/").parse().into_diagnostic()?,
				None => {
					warn!(unit, "no container IP discovered, skipping HTTP probe");
					return Ok(false);
				}
			}
		}
		Supervisor::Pm2 => {
			let Some(pm_id) = instance.pm_id else {
				warn!(name = %instance.name, "pm2 instance has no pm_id, skipping HTTP probe");
				return Ok(false);
			};
			match lifecycle::pm2_port_for(pm_id)? {
				Some(port) => format!("http://127.0.0.1:{port}/").parse().into_diagnostic()?,
				None => {
					info!(name = %instance.name, pm_id, "no PORT in pm2 env, skipping HTTP probe");
					return Ok(false);
				}
			}
		}
	};
	probe_until_ready(client, &url).await;
	Ok(true)
}

/// Retry `url` every 500ms until it returns a non-5xx response. Never gives
/// up. Used for the per-instance readiness probe in the rolling restart,
/// where the container is guaranteed to come up (or the operator can ctrl+c).
async fn probe_until_ready(client: &Client, url: &Url) {
	loop {
		match probe_once(client, url).await {
			Ok(()) => {
				debug!(%url, "probe OK");
				return;
			}
			Err(err) => {
				debug!(%url, err = %err, "probe not ready, retrying");
				tokio::time::sleep(Duration::from_millis(500)).await;
			}
		}
	}
}

/// Bounded probe used for the post-restart `--check-url` end-to-end check.
/// Retries with the same 500ms cadence but bails after `timeout` — unlike
/// the per-instance probe, a failure here is an actual operator-facing
/// result (the user explicitly asked us to verify the URL).
async fn probe_url(client: &Client, url: &Url, timeout: Duration) -> Result<()> {
	let deadline = std::time::Instant::now() + timeout;
	loop {
		match probe_once(client, url).await {
			Ok(()) => {
				debug!(%url, "probe OK");
				return Ok(());
			}
			Err(last_err) => {
				if std::time::Instant::now() >= deadline {
					bail!("HTTP probe of {url} failed: {last_err}");
				}
				debug!(%url, err = %last_err, "probe not ready, retrying");
				tokio::time::sleep(Duration::from_millis(500)).await;
			}
		}
	}
}

async fn probe_once(client: &Client, url: &Url) -> std::result::Result<(), String> {
	match client.get(url.clone()).send().await {
		Ok(resp) if !resp.status().is_server_error() => Ok(()),
		Ok(resp) => Err(format!("HTTP {}", resp.status())),
		Err(e) => Err(e.to_string()),
	}
}
