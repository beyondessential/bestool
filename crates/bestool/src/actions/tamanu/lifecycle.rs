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
use tracing::info;

use super::{
	ApiServerKind, TamanuArgs,
	config::load_config,
	find_tamanu, pm2,
	services::{self, Expectation, Supervisor, parse_systemd_unit},
};

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

/// Filter an expectation set by zero or more substring patterns.
///
/// - Empty `names`: returns every expectation unchanged.
/// - Otherwise: an expectation matches if **any** name in `names` is a
///   substring of the expectation's name.
///
/// Returns an error if any name in `names` matched zero expectations
/// (typo safety in multi-name invocations).
pub fn match_names<'a>(
	expectations: &'a [Expectation],
	names: &[&str],
) -> Result<Vec<&'a Expectation>> {
	if names.is_empty() {
		return Ok(expectations.iter().collect());
	}

	let unmatched: Vec<&str> = names
		.iter()
		.copied()
		.filter(|name| !expectations.iter().any(|e| e.name.contains(name)))
		.collect();
	if !unmatched.is_empty() {
		let available: Vec<&str> = expectations.iter().map(|e| e.name).collect();
		bail!(
			"no service matches: {}; available names are: {}",
			unmatched.join(", "),
			available.join(", "),
		);
	}

	let matched: Vec<&Expectation> = expectations
		.iter()
		.filter(|e| names.iter().any(|name| e.name.contains(name)))
		.collect();
	Ok(matched)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::actions::tamanu::services::{Criticality, ExpectedState, Instances};

	fn exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Up,
			criticality: Criticality::Background,
		}
	}

	#[test]
	fn empty_names_returns_everything() {
		let es = [exp("tamanu-api"), exp("tamanu-tasks"), exp("tamanu-sync")];
		let m = match_names(&es, &[]).unwrap();
		assert_eq!(m.len(), 3);
	}

	#[test]
	fn single_name_substring_matches() {
		let es = [exp("tamanu-central-api"), exp("tamanu-central-tasks")];
		let m = match_names(&es, &["api"]).unwrap();
		assert_eq!(m.len(), 1);
		assert_eq!(m[0].name, "tamanu-central-api");
	}

	#[test]
	fn multi_name_union() {
		let es = [
			exp("tamanu-central-api"),
			exp("tamanu-central-tasks"),
			exp("tamanu-central-fhir-resolve"),
		];
		let m = match_names(&es, &["api", "fhir"]).unwrap();
		assert_eq!(m.len(), 2);
		assert_eq!(
			m.iter().map(|e| e.name).collect::<Vec<_>>(),
			vec!["tamanu-central-api", "tamanu-central-fhir-resolve"],
		);
	}

	#[test]
	fn zero_match_name_bails() {
		let es = [exp("tamanu-api"), exp("tamanu-tasks")];
		let err = match_names(&es, &["nope"]).unwrap_err();
		let msg = format!("{err}");
		assert!(msg.contains("nope"), "error should name the bad pattern: {msg}");
		assert!(msg.contains("tamanu-api"), "error should list available: {msg}");
	}

	#[test]
	fn mixed_match_and_no_match_still_bails() {
		// One typo in a multi-name invocation should bail rather than silently
		// drop the bad pattern and process the rest.
		let es = [exp("tamanu-api"), exp("tamanu-tasks")];
		let err = match_names(&es, &["api", "nope"]).unwrap_err();
		let msg = format!("{err}");
		assert!(msg.contains("nope"), "error should name the bad pattern: {msg}");
	}

	#[test]
	fn name_substring_can_match_multiple() {
		let es = [
			exp("tamanu-central-fhir-resolve"),
			exp("tamanu-central-fhir-refresh"),
			exp("tamanu-api"),
		];
		let m = match_names(&es, &["fhir"]).unwrap();
		assert_eq!(m.len(), 2);
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
