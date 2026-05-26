//! Shared primitives for the `tamanu` lifecycle subcommands (`start`,
//! `stop`, `restart`, `status`).
//!
//! Discovery, matching, and supervisor (systemd/pm2) dispatch all live
//! here so the four subcommand entry points stay thin.

use std::{
	process::Command,
	thread::sleep,
	time::{Duration, Instant},
};

use miette::{IntoDiagnostic, Result, bail};
use tracing::{debug, info, warn};

use bestool_tamanu::{
	ApiServerKind,
	config::load_config,
	pm2,
	server_info::query_patient_portal_enabled,
	services::{self, Expectation, Supervisor, parse_systemd_unit},
};

use super::{TamanuArgs, find_tamanu};

/// Resolve the supervisor + expectation set for the current host.
///
/// Picks systemd on Linux, pm2 on Windows; bails on other platforms.
/// Loads the tamanu config from the discovered root and asks
/// `services::expected` for the canonical expectation list.
///
/// On Linux/central, opens a short-lived DB connection to read
/// `features.patientPortal` so the patient-portal expectation reflects what
/// Tamanu itself thinks is enabled — without this lookup the expectation is
/// always `Down`, which silently no-ops `bestool tamanu start|restart
/// tamanu-patientportal` on deployments where the flag is on. Falls back to
/// `false` on DB error (matches what the doctor reports in the same case).
pub async fn config_and_expectations(
	tamanu: &TamanuArgs,
) -> Result<(Supervisor, Vec<Expectation>)> {
	let supervisor = if cfg!(target_os = "linux") {
		Supervisor::Systemd
	} else if cfg!(target_os = "windows") {
		Supervisor::Pm2
	} else {
		bail!("tamanu lifecycle commands are only supported on Linux (systemd) and Windows (pm2)");
	};

	let (_, root) = find_tamanu(tamanu)?;
	let config = load_config(&root, None)?;
	let kind = if config.is_facility() {
		ApiServerKind::Facility
	} else {
		ApiServerKind::Central
	};

	// The patient-portal expectation is only emitted on Linux/central; skip
	// the DB round-trip in any other shape.
	let patient_portal_enabled =
		if matches!(supervisor, Supervisor::Systemd) && matches!(kind, ApiServerKind::Central) {
			match bestool_postgres::pool::connect_one(
				&config.database_url(),
				"bestool-tamanu-lifecycle",
			)
			.await
			{
				Ok(client) => query_patient_portal_enabled(&client).await,
				Err(err) => {
					warn!(%err, "could not query features.patientPortal; assuming false");
					false
				}
			}
		} else {
			false
		};

	let expectations = services::expected(supervisor, kind, &config, patient_portal_enabled);
	Ok((supervisor, expectations))
}

/// A live service instance discovered from the supervisor.
///
/// One `Instance` per supervisor entry — for a systemd template unit with
/// instances `@1`, `@2` you get two entries with the same `name` and
/// different `instance` suffixes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
	/// Base service name (no `.service`, no `@`).
	pub name: String,
	/// systemd `@instance` suffix, or `None` on pm2 / singleton units.
	pub instance: Option<String>,
	/// pm2 process id, when discovered via pm2.
	pub pm_id: Option<i64>,
	/// Currently active/online.
	pub running: bool,
}

impl Instance {
	/// systemd unit name (`tamanu-frontend@a.service` /
	/// `tamanu-tasks.service`). Not meaningful for pm2 instances.
	pub fn unit(&self) -> String {
		match &self.instance {
			Some(i) => format!("{}@{}.service", self.name, i),
			None => format!("{}.service", self.name),
		}
	}

	/// Short label for log output.
	pub fn display(&self) -> String {
		match (&self.instance, self.pm_id) {
			(Some(i), _) => format!("{}@{}", self.name, i),
			(None, Some(id)) => format!("{}#{id}", self.name),
			(None, None) => self.name.clone(),
		}
	}
}

/// Enumerate the supervisor's view of currently-known tamanu services.
///
/// Includes both running and non-running entries — discovery doesn't
/// itself filter against expectations; that's `match_instances`.
pub fn discover(supervisor: Supervisor) -> Result<Vec<Instance>> {
	match supervisor {
		Supervisor::Systemd => discover_systemd(),
		Supervisor::Pm2 => discover_pm2().map(|(v, _)| v),
	}
}

fn discover_systemd() -> Result<Vec<Instance>> {
	let output = Command::new("systemctl")
		.args([
			"list-units",
			"--type=service",
			"--all",
			"--no-legend",
			"--plain",
			"--no-pager",
			"tamanu-*.service",
		])
		.output()
		.into_diagnostic()?;
	if !output.status.success() {
		bail!(
			"systemctl list-units failed: {}",
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}

	let stdout = String::from_utf8_lossy(&output.stdout);
	let mut out = Vec::new();
	for line in stdout.lines() {
		let mut parts = line.split_whitespace();
		let (Some(unit), Some(load), Some(active), Some(sub)) =
			(parts.next(), parts.next(), parts.next(), parts.next())
		else {
			continue;
		};
		if load == "not-found" {
			continue;
		}
		let Some((base, instance)) = parse_systemd_unit(unit) else {
			continue;
		};
		let running = active == "active" && (sub == "running" || sub == "exited");
		out.push(Instance {
			name: base.to_string(),
			instance: instance.map(str::to_string),
			pm_id: None,
			running,
		});
	}
	Ok(out)
}

fn discover_pm2() -> Result<(Vec<Instance>, pm2::Source)> {
	let (procs, source) = pm2::list().map_err(|e| miette::miette!("pm2: {e}"))?;
	let mut out = Vec::new();
	for p in procs {
		if !p.name.starts_with("tamanu-") {
			continue;
		}
		out.push(Instance {
			name: p.name,
			instance: None,
			pm_id: p.pm_id,
			running: p.running,
		});
	}
	Ok((out, source))
}

/// Group discovered instances under the expectation each belongs to.
///
/// Instances whose `name` and `instance` suffix match an expectation's
/// `name` + `Instances` shape land under that expectation. Unmatched
/// instances are dropped (they're not "expected", so lifecycle commands
/// don't touch them).
pub fn group_by_expectation<'a>(
	expectations: &'a [&'a Expectation],
	instances: &[Instance],
) -> Vec<(&'a Expectation, Vec<Instance>)> {
	expectations
		.iter()
		.map(|exp| {
			let matches: Vec<Instance> = instances
				.iter()
				.filter(|d| {
					d.name == exp.name && exp.instances.admits_instance(d.instance.as_deref())
				})
				.cloned()
				.collect();
			(*exp, matches)
		})
		.collect()
}

/// On Linux/systemd, re-exec the current process under sudo if not
/// already running as root. On pm2 / Windows this is a no-op — pm2
/// manages permissions itself.
///
/// Re-exec is via `Command::status`: the parent waits for sudo to
/// finish, then exits with sudo's status. We don't use `exec` because
/// it requires unsafe and the indirection cost is negligible for a
/// one-shot lifecycle command.
pub fn ensure_root_or_reexec(supervisor: Supervisor) -> Result<()> {
	if !matches!(supervisor, Supervisor::Systemd) {
		return Ok(());
	}
	if privilege::user::privileged() {
		return Ok(());
	}

	info!("not running as root; re-execing under sudo");
	let args: Vec<String> = std::env::args().collect();
	let status = Command::new("sudo").args(args).status().into_diagnostic()?;
	std::process::exit(status.code().unwrap_or(1));
}

/// Poll the supervisor until every target is running, or the timeout
/// elapses. Targets are unit names (systemd) or process names (pm2),
/// matching the input given to `systemctl start` / `pm2 start`.
pub fn wait_running(supervisor: Supervisor, targets: &[String]) -> Result<()> {
	wait_for(supervisor, targets, true, "active")
}

/// Mirror of `wait_running`: poll until every target is stopped.
pub fn wait_stopped(supervisor: Supervisor, targets: &[String]) -> Result<()> {
	wait_for(supervisor, targets, false, "inactive")
}

/// Issue a stop call to the right supervisor for every target.
///
/// `targets` are supervisor-native identifiers — systemd unit names
/// (`tamanu-foo.service`, `tamanu-foo@1.service`) or pm2 process names.
/// No-op for an empty slice. Bails non-zero on supervisor failure; doesn't
/// itself wait for the stop to complete (use `wait_stopped` afterwards).
pub fn stop_targets(supervisor: Supervisor, targets: &[String]) -> Result<()> {
	if targets.is_empty() {
		return Ok(());
	}
	let (cmd, verb) = match supervisor {
		Supervisor::Systemd => ("systemctl", "stop"),
		Supervisor::Pm2 => ("pm2", "stop"),
	};
	let status = Command::new(cmd)
		.arg(verb)
		.args(targets)
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("{cmd} {verb} failed: exit {status}");
	}
	Ok(())
}

/// Restart a batch of pm2 targets by stop-then-start with a short pause
/// between, rather than `pm2 restart`. `targets` are pm2-acceptable
/// identifiers (process names, or pm_ids stringified).
///
/// `pm2 restart` has been observed to occasionally leak the previous
/// node process — it shows up as no longer owned by pm2 (and no longer in
/// `pm2 list`) but still holding the TCP port, causing the freshly
/// started replacement to either fail to bind or sit alongside the
/// zombie. Splitting into explicit stop and start with ~1s in between
/// gives the old process time to exit and release its handles before pm2
/// hands them to the new one. No-op for an empty slice.
pub fn pm2_restart_targets(targets: &[String]) -> Result<()> {
	if targets.is_empty() {
		return Ok(());
	}
	let stop = Command::new("pm2")
		.arg("stop")
		.args(targets)
		.status()
		.into_diagnostic()?;
	if !stop.success() {
		bail!("pm2 stop failed: exit {stop}");
	}
	sleep(Duration::from_secs(1));
	let start = Command::new("pm2")
		.arg("start")
		.args(targets)
		.status()
		.into_diagnostic()?;
	if !start.success() {
		bail!("pm2 start failed: exit {start}");
	}
	Ok(())
}

/// Delete (i.e. unregister) a batch of pm2 processes. pm2's analogue of
/// `systemctl disable`: removes the process entry from pm2's list entirely,
/// so it won't be picked up by `pm2 resurrect` after the next pm2 restart
/// and won't show up in `pm2 list`. Implies stopping the process if it was
/// running. No-op for an empty slice.
///
/// More aggressive than systemd's `disable`: there's no plain "don't
/// auto-start" toggle on pm2, so re-bringing-up requires the ops setup
/// playbook to re-register the process via the ecosystem file.
pub fn delete_pm2(names: &[String]) -> Result<()> {
	if names.is_empty() {
		return Ok(());
	}
	let status = Command::new("pm2")
		.arg("delete")
		.args(names)
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("pm2 delete failed: exit {status}");
	}
	Ok(())
}

/// Disable a batch of systemd units. No-op for an empty slice. Errors
/// bubble up — callers that want best-effort behaviour should filter the
/// list with `systemd_is_enabled` first (the typical pattern).
pub fn disable_systemd_units(units: &[String]) -> Result<()> {
	if units.is_empty() {
		return Ok(());
	}
	let status = Command::new("systemctl")
		.arg("disable")
		.args(units)
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("systemctl disable failed: exit {status}");
	}
	Ok(())
}

fn wait_for(
	supervisor: Supervisor,
	targets: &[String],
	want_running: bool,
	state_label: &str,
) -> Result<()> {
	let deadline = Instant::now() + Duration::from_secs(60);
	let interval = Duration::from_millis(500);
	loop {
		let all_match = targets
			.iter()
			.all(|t| is_running(supervisor, t) == want_running);
		if all_match {
			return Ok(());
		}
		if Instant::now() >= deadline {
			let still_wrong: Vec<&str> = targets
				.iter()
				.filter(|t| is_running(supervisor, t) != want_running)
				.map(String::as_str)
				.collect();
			bail!(
				"timed out after 60s waiting for {} to become {state_label}",
				still_wrong.join(", ")
			);
		}
		sleep(interval);
	}
}

/// Restart a single instance, identified by its supervisor-native key
/// (systemd unit name, or pm2 pm_id).
pub fn restart_one(supervisor: Supervisor, instance: &Instance) -> Result<()> {
	match supervisor {
		Supervisor::Systemd => {
			let status = Command::new("systemctl")
				.args(["restart", &instance.unit()])
				.status()
				.into_diagnostic()?;
			if !status.success() {
				bail!("systemctl restart {} failed: {status}", instance.unit());
			}
			Ok(())
		}
		Supervisor::Pm2 => {
			let id = instance
				.pm_id
				.ok_or_else(|| miette::miette!("pm2 instance {} has no pm_id", instance.name))?;
			pm2_restart_targets(&[id.to_string()])
		}
	}
}

/// Wait for one specific instance to be running again after a restart.
///
/// For systemd, polls `systemctl is-active`. For pm2, polls jlist and
/// matches by `pm_id` (so we can distinguish individual processes that
/// share a name).
pub fn wait_running_one(supervisor: Supervisor, instance: &Instance, timeout: Duration) -> Result<()> {
	let deadline = Instant::now() + timeout;
	let interval = Duration::from_millis(500);
	loop {
		let up = match supervisor {
			Supervisor::Systemd => is_running(supervisor, &instance.unit()),
			Supervisor::Pm2 => is_pm2_pm_id_online(instance.pm_id),
		};
		if up {
			return Ok(());
		}
		if Instant::now() >= deadline {
			bail!(
				"timed out after {}s waiting for {} to become active",
				timeout.as_secs(),
				instance.display(),
			);
		}
		sleep(interval);
	}
}

fn is_pm2_pm_id_online(pm_id: Option<i64>) -> bool {
	let Some(id) = pm_id else { return false };
	match pm2::list() {
		Ok((procs, _)) => procs.iter().any(|p| p.pm_id == Some(id) && p.running),
		Err(_) => false,
	}
}

/// Reload caddy (Linux: + flush systemd-resolved). Needed after
/// restarting a containerised tamanu service: caddy's upstream list is by
/// hostname, resolved caches IPs, and the restarted container has a new
/// IP. All calls are best-effort: failures are logged but don't bail.
///
/// Uses the platform-native path that reads the on-disk Caddyfile:
///
/// - Linux: `systemctl reload caddy`. We deliberately don't POST the
///   Caddyfile to the admin API here even though it'd work for a
///   single-file config — production deployments split their Caddyfile
///   across `/etc/caddy/conf.d/*` includes, and the safest way to pick
///   up every fragment is to let caddy re-read from disk the same way
///   it did at startup.
/// - Windows: tries the admin API first (`POST localhost:2019/load`
///   with the Caddyfile content) since it's the most reliable when
///   available; falls back to `caddy reload --config <path> --adapter
///   caddyfile` if the admin endpoint is unreachable.
///
/// On Linux, also runs `resolvectl flush-caches` regardless of which
/// reload path actually fired — systemd-resolved caches DNS independently
/// of caddy's upstream list.
///
/// Mirror of the ansible "Reload caddy" handler from #313.
#[cfg(target_os = "linux")]
pub async fn reload_caddy() {
	let status = Command::new("systemctl").args(["reload", "caddy"]).status();
	match status {
		Ok(s) if s.success() => debug!("caddy reloaded via systemctl"),
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

#[cfg(target_os = "windows")]
pub async fn reload_caddy() {
	let path = r"C:\Caddy\Caddyfile";
	match reload_caddy_via_admin_api(path).await {
		Ok(()) => {
			debug!("caddy reloaded via admin API");
			return;
		}
		Err(err) => debug!(%err, "caddy admin API unreachable, falling back to CLI"),
	}
	// `caddy reload` reads the file, adapts it, and POSTs to the admin
	// API on the caddy process's behalf. Same end-state as the admin-API
	// path above; this fallback handles the case where caddy is running
	// but the admin endpoint is locked down or relocated.
	let status = Command::new("caddy")
		.args(["reload", "--config", path, "--adapter", "caddyfile"])
		.status();
	match status {
		Ok(s) if s.success() => debug!("caddy reloaded via CLI"),
		Ok(s) => warn!("caddy reload exited with {s}"),
		Err(e) => warn!("could not run caddy reload: {e}"),
	}
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub async fn reload_caddy() {
	debug!("caddy reload is unsupported on this OS");
}

/// POST the Caddyfile content to Caddy's admin API at the default
/// `localhost:2019/load` endpoint with `Content-Type: text/caddyfile`,
/// telling Caddy to adapt + load it in-process. Returns an error string
/// (with enough context to debug) if the file can't be read, the API
/// doesn't respond, or it returns a non-2xx — the caller logs at debug
/// and falls back to the platform CLI.
#[cfg(target_os = "windows")]
async fn reload_caddy_via_admin_api(path: &str) -> std::result::Result<(), String> {
	let content = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
	let client = reqwest::Client::builder()
		.timeout(Duration::from_secs(5))
		.build()
		.map_err(|e| format!("build client: {e}"))?;
	let resp = client
		.post("http://localhost:2019/load")
		.header("Content-Type", "text/caddyfile")
		.body(content)
		.send()
		.await
		.map_err(|e| format!("POST localhost:2019/load: {e}"))?;
	if !resp.status().is_success() {
		let status = resp.status();
		let body = resp.text().await.unwrap_or_default();
		return Err(format!("admin API returned {status}: {body}"));
	}
	Ok(())
}

/// Look up the netavark IP of the podman container backing a systemd
/// unit. Returns None if there's no matching container or no IP yet
/// (e.g. container not finished starting). Mirror of #313's helper.
pub fn container_ip_for_unit(unit: &str) -> Result<Option<std::net::IpAddr>> {
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
		.map(|s| s.parse::<std::net::IpAddr>())
		.transpose()
		.into_diagnostic()?;
	Ok(ip)
}

/// Read pm2's view of an instance to find its listening port from the
/// `PORT` env var. Returns None if pm2 doesn't expose one — e.g.
/// workers that don't open a socket.
pub fn pm2_port_for(pm_id: i64) -> Result<Option<u16>> {
	let output = Command::new("pm2").arg("jlist").output().into_diagnostic()?;
	if !output.status.success() {
		bail!(
			"pm2 jlist failed: {}",
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	let entries: Vec<serde_json::Value> =
		serde_json::from_slice(&output.stdout).into_diagnostic()?;
	let port = entries
		.iter()
		.find(|p| p["pm_id"].as_i64() == Some(pm_id))
		.and_then(|p| p["pm2_env"].get("env"))
		.and_then(|env| env.get("PORT"))
		.and_then(|v| {
			v.as_str()
				.and_then(|s| s.parse().ok())
				.or_else(|| v.as_u64().and_then(|n| u16::try_from(n).ok()))
		});
	Ok(port)
}

fn is_running(supervisor: Supervisor, target: &str) -> bool {
	match supervisor {
		Supervisor::Systemd => Command::new("systemctl")
			.args(["is-active", "--quiet", target])
			.status()
			.map(|s| s.success())
			.unwrap_or(false),
		Supervisor::Pm2 => match pm2::list() {
			Ok((procs, _)) => procs.iter().any(|p| p.name == target && p.running),
			Err(_) => false,
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use bestool_tamanu::services::{ExpectedState, Instances};

	fn exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Up,
			reason: "test".into(),
			legacy: false,
			behind_caddy: false,
		}
	}

	fn templated_exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::NumericAtLeast(2),
			state: ExpectedState::Up,
			reason: "test".into(),
			legacy: false,
			behind_caddy: false,
		}
	}

	fn inst(name: &str, instance: Option<&str>, running: bool) -> Instance {
		Instance {
			name: name.to_string(),
			instance: instance.map(str::to_string),
			pm_id: None,
			running,
		}
	}

	#[test]
	fn group_by_expectation_collects_matching_instances() {
		let api = templated_exp("tamanu-central-api");
		let tasks = exp("tamanu-central-tasks");
		let expectations = [&api, &tasks];
		let instances = vec![
			inst("tamanu-central-api", Some("1"), true),
			inst("tamanu-central-api", Some("2"), true),
			inst("tamanu-central-tasks", None, true),
			inst("tamanu-orphan", None, true), // unrelated, dropped
		];
		let groups = group_by_expectation(&expectations, &instances);
		assert_eq!(groups.len(), 2);
		assert_eq!(groups[0].0.name, "tamanu-central-api");
		assert_eq!(groups[0].1.len(), 2);
		assert_eq!(groups[1].0.name, "tamanu-central-tasks");
		assert_eq!(groups[1].1.len(), 1);
	}

	#[test]
	fn instance_unit_and_display() {
		let templated = inst("tamanu-frontend", Some("a"), true);
		assert_eq!(templated.unit(), "tamanu-frontend@a.service");
		assert_eq!(templated.display(), "tamanu-frontend@a");

		let singleton = inst("tamanu-tasks", None, true);
		assert_eq!(singleton.unit(), "tamanu-tasks.service");
		assert_eq!(singleton.display(), "tamanu-tasks");

		let pm2 = Instance {
			name: "tamanu-api".into(),
			instance: None,
			pm_id: Some(3),
			running: true,
		};
		assert_eq!(pm2.display(), "tamanu-api#3");
	}
}
