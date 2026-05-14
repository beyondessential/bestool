use std::{
	net::IpAddr,
	process::Command,
	time::{Duration, Instant},
};

use clap::Parser;
use miette::{IntoDiagnostic, Result, bail};
use regex::Regex;
use reqwest::{Client, Url};
use serde::Deserialize;
use tracing::{debug, info, warn};

use crate::actions::Context;

use super::{ApiServerKind, TamanuArgs, find_package, find_tamanu};

/// Restart Tamanu services one at a time.
///
/// On Linux, restarts the running `tamanu-{kind}-*` systemd units, plus
/// shared ones (`tamanu-frontend@*`, `tamanu-patientportal`). After each
/// restart, the strict readiness signal is:
///
///   1. systemd reports the unit `active`
///   2. the unit's podman container responds on port 3000 (HTTP services only;
///      workers like `*-tasks` and `*-fhir-*` skip this step)
///
/// Then caddy is reloaded and systemd-resolved flushed (so caddy picks up the
/// new container IP), a configurable cooldown is awaited, and optionally an
/// external HTTP URL is probed.
///
/// On Windows, restarts every `online` pm2 process one pm_id at a time (so
/// scaled apps like `tamanu-api` roll instance-by-instance, not all at once).
/// Strict readiness is `pm2` reporting `online` plus an HTTP probe of
/// `http://127.0.0.1:<PORT>/` where `<PORT>` is the process's resolved `PORT`
/// env var. Processes without a `PORT` (workers like `tamanu-tasks`,
/// `tamanu-sync`, `tamanu-fhir-*`) skip the HTTP probe.
///
/// Examples:
///   bestool tamanu reload
///   bestool tamanu reload --filter '^tamanu-frontend@'
///   bestool tamanu reload --check-url https://central.example.org --wait 15
#[derive(Debug, Clone, Parser)]
pub struct ReloadArgs {
	/// Override the detected server kind.
	#[arg(long, value_enum)]
	pub kind: Option<ApiServerKind>,

	/// Seconds to wait between restarts.
	#[arg(long, default_value = "10")]
	pub wait: u64,

	/// External HTTP URL to probe after each restart.
	///
	/// If set, after each service is restarted and reported ready, this URL is
	/// requested. A connection failure or 5xx response aborts the rollout.
	/// This is independent of the strict per-container probe (see --no-strict).
	#[arg(long)]
	pub check_url: Option<Url>,

	/// Skip the strict HTTP probe after each restart.
	///
	/// On Linux the strict probe hits the unit's podman container directly on
	/// port 3000. On Windows it hits `http://127.0.0.1:<PORT>/` where `<PORT>`
	/// is the pm2 process's resolved `PORT` env var (workers without a PORT
	/// are skipped). With strict off, readiness only checks that the process
	/// manager reports the service running.
	#[arg(long)]
	pub no_strict: bool,

	/// Per-step timeout in seconds (readiness polling and HTTP probe).
	#[arg(long, default_value = "30")]
	pub timeout: u64,

	/// Only restart services whose name matches this regex.
	#[arg(long)]
	pub filter: Option<Regex>,

	/// Don't reload caddy / flush resolved between restarts.
	#[arg(long)]
	pub no_caddy_reload: bool,

	/// Print the plan and exit without restarting anything.
	#[arg(long)]
	pub dry_run: bool,
}

#[derive(Copy, Clone, Debug)]
enum Backend {
	Systemd,
	Pm2,
}

#[derive(Debug, Clone)]
enum Service {
	Systemd(String),
	Pm2(Pm2Proc),
}

impl Service {
	fn label(&self) -> String {
		match self {
			Self::Systemd(name) => name.clone(),
			Self::Pm2(p) => format!("{} #{}", p.name, p.pm_id),
		}
	}
}

pub async fn run(ctx: Context<TamanuArgs, ReloadArgs>) -> Result<()> {
	let backend = detect_backend()?;
	let kind = resolve_kind(&ctx, backend)?;
	info!(?backend, ?kind, "rolling tamanu services");

	let services: Vec<Service> = match backend {
		Backend::Systemd => list_services_systemd(kind, ctx.args_sub.filter.as_ref())?
			.into_iter()
			.map(Service::Systemd)
			.collect(),
		Backend::Pm2 => list_services_pm2(ctx.args_sub.filter.as_ref())?
			.into_iter()
			.map(Service::Pm2)
			.collect(),
	};

	if services.is_empty() {
		bail!("no running tamanu services match the selection");
	}

	info!("will restart {} service(s):", services.len());
	for s in &services {
		info!("  - {}", s.label());
	}

	if ctx.args_sub.dry_run {
		return Ok(());
	}

	let timeout = Duration::from_secs(ctx.args_sub.timeout);
	let wait = Duration::from_secs(ctx.args_sub.wait);

	let http = if ctx.args_sub.check_url.is_some() {
		Some(
			Client::builder()
				.timeout(timeout)
				.build()
				.into_diagnostic()?,
		)
	} else {
		None
	};

	let strict_client = if !ctx.args_sub.no_strict {
		Some(
			Client::builder()
				.timeout(Duration::from_secs(2))
				.build()
				.into_diagnostic()?,
		)
	} else {
		None
	};

	let total = services.len();
	for (idx, service) in services.iter().enumerate() {
		let n = idx + 1;
		let label = service.label();
		info!("[{n}/{total}] restarting {label}");
		match service {
			Service::Systemd(name) => {
				restart_systemd(name)?;
				wait_active_systemd(name, timeout).await?;
				if let Some(client) = &strict_client {
					wait_container_ready(name, client, timeout).await?;
				}
				if !ctx.args_sub.no_caddy_reload {
					reload_caddy();
				}
			}
			Service::Pm2(proc) => {
				restart_pm2(proc.pm_id)?;
				wait_online_pm2(proc.pm_id, timeout).await?;
				if let Some(client) = &strict_client {
					wait_pm2_port_ready(proc, client, timeout).await?;
				}
			}
		}

		if let (Some(http), Some(url)) = (&http, &ctx.args_sub.check_url) {
			probe_http(http, url, timeout).await?;
		}

		if n < total && !wait.is_zero() {
			info!("waiting {}s before next restart", wait.as_secs());
			tokio::time::sleep(wait).await;
		}
	}

	info!("rolled {total} service(s)");
	Ok(())
}

fn resolve_kind(ctx: &Context<TamanuArgs, ReloadArgs>, backend: Backend) -> Result<ApiServerKind> {
	if let Some(kind) = ctx.args_sub.kind {
		return Ok(kind);
	}
	if let Ok((_, root)) = find_tamanu(&ctx.args_top) {
		return Ok(find_package(root));
	}
	let detected = match backend {
		Backend::Systemd => detect_kind_systemd()?,
		Backend::Pm2 => detect_kind_pm2()?,
	};
	if let Some(kind) = detected {
		return Ok(kind);
	}
	bail!("could not detect server kind: pass --kind central|facility")
}

fn detect_kind_systemd() -> Result<Option<ApiServerKind>> {
	let units = list_systemd_units()?;
	let has_central = units
		.iter()
		.any(|u| u.unit.starts_with("tamanu-central-") && u.active == "active");
	let has_facility = units
		.iter()
		.any(|u| u.unit.starts_with("tamanu-facility-") && u.active == "active");
	match (has_central, has_facility) {
		(true, false) => Ok(Some(ApiServerKind::Central)),
		(false, true) => Ok(Some(ApiServerKind::Facility)),
		(true, true) => bail!("both central and facility services are running; pass --kind"),
		(false, false) => Ok(None),
	}
}

/// `tamanu-sync` only runs on facilities, so its presence in pm2 implies a
/// facility install. This is a last-resort fallback after the config-based
/// detection in [`find_tamanu`].
fn detect_kind_pm2() -> Result<Option<ApiServerKind>> {
	let procs = pm2_jlist()?;
	if procs.iter().any(|p| p.name == "tamanu-sync") {
		Ok(Some(ApiServerKind::Facility))
	} else if procs.iter().any(|p| p.name.starts_with("tamanu-")) {
		Ok(Some(ApiServerKind::Central))
	} else {
		Ok(None)
	}
}

fn detect_backend() -> Result<Backend> {
	if cfg!(windows) {
		return Ok(Backend::Pm2);
	}
	if cmd_available("systemctl", "--version") {
		Ok(Backend::Systemd)
	} else if cmd_available(pm2_cmd(), "--version") {
		Ok(Backend::Pm2)
	} else {
		bail!("neither systemctl nor pm2 is available on this host");
	}
}

fn cmd_available(cmd: &str, arg: &str) -> bool {
	Command::new(cmd)
		.arg(arg)
		.output()
		.map(|o| o.status.success())
		.unwrap_or(false)
}

fn pm2_cmd() -> &'static str {
	if cfg!(windows) { "pm2.cmd" } else { "pm2" }
}

#[derive(Debug, Deserialize)]
struct SystemdUnit {
	unit: String,
	active: String,
}

fn list_systemd_units() -> Result<Vec<SystemdUnit>> {
	let output = Command::new("systemctl")
		.args([
			"list-units",
			"--type=service",
			"--all",
			"--no-legend",
			"--plain",
			"--no-pager",
			"-o",
			"json",
		])
		.output()
		.into_diagnostic()?;
	if !output.status.success() {
		bail!(
			"systemctl list-units failed: {}",
			String::from_utf8_lossy(&output.stderr)
		);
	}
	serde_json::from_slice(&output.stdout).into_diagnostic()
}

fn list_services_systemd(kind: ApiServerKind, filter: Option<&Regex>) -> Result<Vec<String>> {
	let units = list_systemd_units()?;
	let kind_prefix = match kind {
		ApiServerKind::Central => "tamanu-central-",
		ApiServerKind::Facility => "tamanu-facility-",
	};

	let mut services: Vec<String> = units
		.into_iter()
		.filter(|u| u.active == "active")
		.filter(|u| {
			let name = u.unit.trim_end_matches(".service");
			(name.starts_with(kind_prefix)
				|| name.starts_with("tamanu-frontend@")
				|| name == "tamanu-patientportal")
				&& !u.unit.ends_with("@.service")
		})
		.map(|u| u.unit)
		.filter(|n| filter.is_none_or(|r| r.is_match(n)))
		.collect();

	services.sort();
	Ok(services)
}

fn restart_systemd(name: &str) -> Result<()> {
	let status = Command::new("systemctl")
		.args(["restart", name])
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("systemctl restart {name} exited with {status}");
	}
	Ok(())
}

async fn wait_active_systemd(name: &str, timeout: Duration) -> Result<()> {
	let deadline = Instant::now() + timeout;
	loop {
		let output = Command::new("systemctl")
			.args(["is-active", name])
			.output()
			.into_diagnostic()?;
		let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
		debug!(service = %name, state = %state, "systemctl is-active");
		if state == "active" {
			return Ok(());
		}
		if Instant::now() >= deadline {
			bail!(
				"{name} did not become active within {}s (current state: {state})",
				timeout.as_secs()
			);
		}
		tokio::time::sleep(Duration::from_millis(500)).await;
	}
}

/// Mirror the ansible "Reload caddy" handler: `systemctl reload caddy` plus
/// `resolvectl flush-caches`. Without this, caddy keeps routing to the
/// container IP of the process we just restarted. Best-effort: warn and
/// continue on failure.
fn reload_caddy() {
	let status = Command::new("systemctl")
		.args(["reload", "caddy"])
		.status();
	match status {
		Ok(s) if s.success() => debug!("caddy reloaded"),
		Ok(s) => warn!("systemctl reload caddy exited with {s}"),
		Err(e) => warn!("could not reload caddy: {e}"),
	}
	let status = Command::new("resolvectl").arg("flush-caches").status();
	match status {
		Ok(s) if s.success() => debug!("resolvectl flush-caches OK"),
		Ok(s) => warn!("resolvectl flush-caches exited with {s}"),
		Err(e) => debug!("resolvectl not available: {e}"),
	}
}

#[derive(Debug, Clone, Deserialize)]
struct Pm2Proc {
	pm_id: u32,
	name: String,
	pm2_env: Pm2Env,
}

#[derive(Debug, Clone, Deserialize)]
struct Pm2Env {
	status: String,
	#[serde(default)]
	env: serde_json::Map<String, serde_json::Value>,
}

impl Pm2Proc {
	fn port(&self) -> Option<u16> {
		let v = self.pm2_env.env.get("PORT")?;
		v.as_str()
			.and_then(|s| s.parse().ok())
			.or_else(|| v.as_u64().and_then(|n| u16::try_from(n).ok()))
	}
}

fn pm2_jlist() -> Result<Vec<Pm2Proc>> {
	let output = Command::new(pm2_cmd())
		.arg("jlist")
		.output()
		.into_diagnostic()?;
	if !output.status.success() {
		bail!(
			"pm2 jlist failed: {}",
			String::from_utf8_lossy(&output.stderr)
		);
	}
	serde_json::from_slice(&output.stdout).into_diagnostic()
}

fn list_services_pm2(filter: Option<&Regex>) -> Result<Vec<Pm2Proc>> {
	let mut procs: Vec<Pm2Proc> = pm2_jlist()?
		.into_iter()
		.filter(|p| p.pm2_env.status == "online")
		.filter(|p| filter.is_none_or(|r| r.is_match(&p.name)))
		.collect();
	procs.sort_by(|a, b| a.name.cmp(&b.name).then(a.pm_id.cmp(&b.pm_id)));
	Ok(procs)
}

fn restart_pm2(pm_id: u32) -> Result<()> {
	let status = Command::new(pm2_cmd())
		.args(["restart", &pm_id.to_string()])
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("pm2 restart {pm_id} exited with {status}");
	}
	Ok(())
}

async fn wait_online_pm2(pm_id: u32, timeout: Duration) -> Result<()> {
	let deadline = Instant::now() + timeout;
	loop {
		let procs = pm2_jlist()?;
		let status = procs
			.iter()
			.find(|p| p.pm_id == pm_id)
			.map(|p| p.pm2_env.status.as_str())
			.unwrap_or("missing");
		debug!(pm_id, status, "pm2 status");
		if status == "online" {
			return Ok(());
		}
		if Instant::now() >= deadline {
			bail!(
				"pm2 process #{pm_id} did not become online within {}s (current status: {status})",
				timeout.as_secs()
			);
		}
		tokio::time::sleep(Duration::from_millis(500)).await;
	}
}

async fn wait_pm2_port_ready(proc: &Pm2Proc, client: &Client, timeout: Duration) -> Result<()> {
	let Some(port) = proc.port() else {
		debug!(name = %proc.name, pm_id = proc.pm_id, "no PORT env; skipping HTTP probe");
		return Ok(());
	};
	let url = format!("http://127.0.0.1:{port}/");
	let deadline = Instant::now() + timeout;
	loop {
		let last_err = match client.get(&url).send().await {
			Ok(resp) => {
				debug!(name = %proc.name, pm_id = proc.pm_id, status = %resp.status(), %url, "pm2 process responded");
				return Ok(());
			}
			Err(e) => e.to_string(),
		};
		if Instant::now() >= deadline {
			bail!(
				"{} #{} did not start responding on {url} within {}s: {last_err}",
				proc.name,
				proc.pm_id,
				timeout.as_secs()
			);
		}
		tokio::time::sleep(Duration::from_millis(500)).await;
	}
}

/// Skip the per-container HTTP probe for known workers — they don't listen on
/// a port. Matches the quadlets in ops repo (no NetworkAlias on these).
fn is_worker_unit(unit: &str) -> bool {
	let name = unit.trim_end_matches(".service");
	name.ends_with("-tasks") || name.contains("-fhir-")
}

async fn wait_container_ready(unit: &str, client: &Client, timeout: Duration) -> Result<()> {
	if is_worker_unit(unit) {
		debug!(service = %unit, "worker service; skipping HTTP probe");
		return Ok(());
	}
	let Some(ip) = container_ip_for_unit(unit)? else {
		warn!(service = %unit, "container IP not found, falling back to is-active only");
		return Ok(());
	};
	let url = format!("http://{ip}:3000/");
	let deadline = Instant::now() + timeout;
	loop {
		let last_err = match client.get(&url).send().await {
			Ok(resp) => {
				debug!(service = %unit, status = %resp.status(), %url, "container responded");
				return Ok(());
			}
			Err(e) => e.to_string(),
		};
		if Instant::now() >= deadline {
			bail!(
				"{unit} did not start responding on {url} within {}s: {last_err}",
				timeout.as_secs()
			);
		}
		tokio::time::sleep(Duration::from_millis(500)).await;
	}
}

fn container_ip_for_unit(unit: &str) -> Result<Option<IpAddr>> {
	let ps = Command::new("podman")
		.args([
			"ps",
			"--filter",
			&format!("label=PODMAN_SYSTEMD_UNIT={unit}"),
			"--format",
			"json",
		])
		.output();
	let ps = match ps {
		Ok(o) if o.status.success() => o,
		Ok(o) => {
			warn!(
				"podman ps failed: {}",
				String::from_utf8_lossy(&o.stderr).trim()
			);
			return Ok(None);
		}
		Err(e) => {
			debug!("podman not available: {e}");
			return Ok(None);
		}
	};

	let entries: Vec<serde_json::Value> = serde_json::from_slice(&ps.stdout).into_diagnostic()?;
	let Some(id) = entries.first().and_then(|c| c["Id"].as_str()) else {
		return Ok(None);
	};

	let inspect = Command::new("podman")
		.args(["inspect", id])
		.output()
		.into_diagnostic()?;
	if !inspect.status.success() {
		bail!(
			"podman inspect failed: {}",
			String::from_utf8_lossy(&inspect.stderr).trim()
		);
	}
	let inspects: Vec<serde_json::Value> =
		serde_json::from_slice(&inspect.stdout).into_diagnostic()?;
	let ip = inspects
		.first()
		.and_then(|c| c["NetworkSettings"]["Networks"].as_object())
		.and_then(|nets| nets.values().find_map(|n| n["IPAddress"].as_str()))
		.filter(|s| !s.is_empty())
		.map(|s| s.parse::<IpAddr>())
		.transpose()
		.into_diagnostic()?;
	Ok(ip)
}

async fn probe_http(client: &Client, url: &Url, timeout: Duration) -> Result<()> {
	let deadline = Instant::now() + timeout;
	loop {
		let last_err = match client.get(url.clone()).send().await {
			Ok(resp) if !resp.status().is_server_error() => {
				debug!(status = %resp.status(), url = %url, "probe OK");
				return Ok(());
			}
			Ok(resp) => format!("HTTP {}", resp.status()),
			Err(e) => e.to_string(),
		};
		if Instant::now() >= deadline {
			bail!("HTTP probe of {url} failed: {last_err}");
		}
		warn!(url = %url, err = %last_err, "HTTP probe not ready, retrying");
		tokio::time::sleep(Duration::from_millis(500)).await;
	}
}
