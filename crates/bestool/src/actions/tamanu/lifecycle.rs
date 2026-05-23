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
	services::{self, Expectation, Supervisor, parse_systemd_unit},
};

use super::{TamanuArgs, find_tamanu};

/// Resolve the supervisor + expectation set for the current host.
///
/// Picks systemd on Linux, pm2 on Windows; bails on other platforms.
/// Loads the tamanu config from the discovered root and asks
/// `services::expected` for the canonical expectation list.
pub fn config_and_expectations(tamanu: &TamanuArgs) -> Result<(Supervisor, Vec<Expectation>)> {
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

	let expectations = services::expected(supervisor, kind, &config);
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
			let status = Command::new("pm2")
				.args(["restart", &id.to_string()])
				.status()
				.into_diagnostic()?;
			if !status.success() {
				bail!("pm2 restart {id} failed: {status}");
			}
			Ok(())
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

/// Reload caddy + flush systemd-resolved. Needed after restarting a
/// containerised tamanu service: caddy's upstream list is by hostname,
/// resolved caches IPs, and the restarted container has a new IP. Both
/// calls are best-effort: failures are logged but don't bail.
///
/// Mirror of the ansible "Reload caddy" handler from #313.
pub fn reload_caddy() {
	let status = Command::new("systemctl").args(["reload", "caddy"]).status();
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
	use bestool_tamanu::services::{Criticality, ExpectedState, Instances};

	fn exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Up,
			criticality: Criticality::Background,
		}
	}

	fn templated_exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::NumericAtLeast(2),
			state: ExpectedState::Up,
			criticality: Criticality::Critical,
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
