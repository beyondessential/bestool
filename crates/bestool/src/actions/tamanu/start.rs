use std::{
	collections::HashSet,
	process::{Child, Command, Stdio},
	time::{Duration, Instant},
};

use clap::Parser;
use miette::{IntoDiagnostic, Result, WrapErr, bail};
use tracing::{debug, info, warn};

use bestool_tamanu::{
	services::{self, ExpectedState, Expectation, Supervisor},
	systemd,
};

use crate::actions::{
	Context,
	tamanu::{
		TamanuArgs,
		lifecycle::{self, Instance, WaitForDb},
		probe,
	},
};

/// Normalise tamanu services to the expected running state.
///
/// Default mode does both halves: stops (and disables, on systemd) any
/// service we expect to be `Down` that's currently running or enabled,
/// then starts any `Up` service that's currently missing or short. With
/// `--up-only` it behaves like the previous `start`-only version: just
/// brings up missing services without touching anything else.
///
/// Idempotent: services already in the expected state are left alone.
/// Use `tamanu status` first to see what's drifted.
///
/// After starting, the behind-caddy HTTP services (API, frontend, patient
/// portal) are probed for readiness within a one-minute budget
/// (`--probe-timeout`); if any don't come up, `start` bails. Pass
/// `--no-probe-http` to skip the check. With `--logs`, the tamanu service
/// logs are streamed for the duration of the start so the operator can
/// watch startup.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct StartArgs {
	/// Limit to expectations whose name contains any of these substrings.
	/// No names = consider every expectation.
	pub names: Vec<String>,

	/// Skip the stop/disable phase: only bring up missing Up services,
	/// leave any drifted Down services as-is. Useful when you want to
	/// avoid touching a service that's running but shouldn't be (e.g.
	/// because you're mid-investigation).
	#[arg(long)]
	pub up_only: bool,

	/// Skip the post-start HTTP readiness probe.
	#[arg(long)]
	pub no_probe_http: bool,

	/// How long to wait for started services to pass their readiness probe before bailing.
	#[arg(long, default_value = "1m", value_parser = probe::parse_duration)]
	pub probe_timeout: Duration,

	/// Stream tamanu service logs while starting.
	#[arg(long)]
	pub logs: bool,
}

pub async fn run(args: StartArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();

	// `tamanu start` is invoked at boot (systemd unit ordering puts it
	// before tamanu.target but doesn't gate on postgres readiness), so
	// wait for the DB to accept connections before reading the
	// `features.patientPortal` flag. Without this, a slow-starting
	// postgres makes the portal expectation flip to Down and we silently
	// skip starting it.
	let (supervisor, expectations) =
		lifecycle::config_and_expectations(tamanu, WaitForDb::Yes).await?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = services::match_names(&expectations, &names)?;
	lifecycle::warn_unknown_expectations(&matched);
	let discovered = lifecycle::discover(supervisor).await?;
	let groups = lifecycle::group_by_expectation(&matched, &discovered);

	let stop_plan = if args.up_only {
		StopPlan::default()
	} else {
		let candidates: HashSet<String> = groups
			.iter()
			.filter(|(exp, _)| matches!(exp.state, ExpectedState::Down))
			.flat_map(|(exp, instances)| {
				let mut units = exp.instances.required_systemd_units(exp.name);
				for inst in instances {
					let u = inst.unit();
					if !units.contains(&u) {
						units.push(u);
					}
				}
				units
			})
			.collect();
		let enabled = if matches!(supervisor, Supervisor::Systemd) {
			systemd::collect_enabled(candidates).await
		} else {
			HashSet::new()
		};
		plan_stop(supervisor, &groups, |unit| enabled.contains(unit))
	};
	let Plan {
		targets,
		started_behind_caddy,
	} = plan_start(supervisor, &groups)?;
	if stop_plan.is_empty() && targets.is_empty() {
		tracing::info!("nothing to do; everything matches expected state");
		return Ok(());
	}

	lifecycle::ensure_root_or_reexec(supervisor)?;

	// Spawn the log follower (if requested) before issuing the start, so the
	// operator sees startup output as it happens. The guard's Drop kills the
	// follower when `run` returns — on success or on a bail from the probe
	// phase below.
	let _follower = if args.logs {
		let log_targets: Vec<String> = groups
			.iter()
			.filter(|(exp, _)| exp.state == ExpectedState::Up)
			.map(|(exp, _)| exp.name.to_string())
			.collect();
		spawn_log_follower(supervisor, &log_targets)
	} else {
		None
	};

	if !stop_plan.is_empty() {
		execute_stop(supervisor, &stop_plan).await?;
	}

	if !targets.is_empty() {
		tracing::info!(?targets, "starting");
		match supervisor {
			Supervisor::Systemd => systemctl_start(&targets).await?,
			Supervisor::Pm2 => pm2_start(&targets)?,
		}
		lifecycle::wait_running(supervisor, &targets).await?;
	}

	// Behind-caddy services (API, frontend, patient portal) reach Caddy by
	// container hostname. When one of those comes up fresh its podman
	// container has a brand-new netavark IP, but Caddy and systemd-resolved
	// still hold the previous one — without a reload the next request hits
	// a stale upstream. `restart` reloads per-instance for critical
	// services; `start` brings everything up in one batch, so a single
	// reload at the end covers them all.
	if started_behind_caddy {
		lifecycle::reload_caddy().await;
	}

	// Verify the behind-caddy HTTP services actually came up. The supervisor
	// reporting the unit active isn't the same as the container accepting
	// connections, so probe each within a single overall budget and bail if
	// any fails to respond in time.
	if started_behind_caddy && !args.no_probe_http {
		let discovered = lifecycle::discover(supervisor).await?;
		let groups = lifecycle::group_by_expectation(&matched, &discovered);
		let to_probe = instances_to_probe(&groups);
		probe_started(supervisor, &to_probe, args.probe_timeout).await?;
	}

	Ok(())
}

/// Select the running, behind-caddy, expected-Up instances to probe after a
/// start. Excludes non-behind-caddy, non-running, and non-Up instances.
fn instances_to_probe(groups: &[(&Expectation, Vec<Instance>)]) -> Vec<Instance> {
	let mut out = Vec::new();
	for (exp, instances) in groups {
		if exp.state != ExpectedState::Up || !exp.behind_caddy {
			continue;
		}
		for inst in instances {
			if inst.running {
				out.push(inst.clone());
			}
		}
	}
	out
}

/// Probe every selected instance for readiness within a single overall
/// deadline. Bails (naming the instance) on the first one that doesn't pass
/// in time. Instances without a constructable probe URL are skipped.
async fn probe_started(
	supervisor: Supervisor,
	instances: &[Instance],
	timeout: Duration,
) -> Result<()> {
	if instances.is_empty() {
		return Ok(());
	}
	let client = probe::http_client()?;
	let deadline = Instant::now() + timeout;
	for inst in instances {
		let Some(url) = probe::instance_probe_url(supervisor, inst)? else {
			debug!(instance = %inst.display(), "no probe URL, skipping readiness check");
			continue;
		};
		let remaining = deadline.saturating_duration_since(Instant::now());
		if remaining.is_zero() {
			bail!(
				"readiness probe budget exhausted before probing {}",
				inst.display()
			);
		}
		info!(instance = %inst.display(), %url, "probing started service for readiness");
		probe::probe_url(&client, &url, remaining)
			.await
			.wrap_err_with(|| format!("{} did not become ready", inst.display()))?;
	}
	Ok(())
}

/// Holds a spawned log-follower child process and kills it on drop, so the
/// follower lives exactly as long as the `start` work and is torn down on
/// both success and bail.
struct LogFollower {
	child: Child,
}

impl Drop for LogFollower {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}

/// Spawn a log follower for the given service base names, inheriting stdio so
/// its output interleaves with `start`'s progress logs. Spawn failures are
/// non-fatal: warn and return `None`.
fn spawn_log_follower(supervisor: Supervisor, targets: &[String]) -> Option<LogFollower> {
	if targets.is_empty() {
		return None;
	}
	let mut cmd = match supervisor {
		Supervisor::Systemd => {
			let mut cmd = Command::new("journalctl");
			cmd.args(["-f", "-n", "0"]);
			for t in targets {
				cmd.arg("-u").arg(format!("{t}*"));
			}
			cmd
		}
		Supervisor::Pm2 => {
			let mut deduped: Vec<&String> = Vec::new();
			for t in targets {
				if !deduped.contains(&t) {
					deduped.push(t);
				}
			}
			let mut cmd = Command::new("pm2");
			cmd.args(["logs", "--lines", "0"]);
			cmd.args(deduped);
			cmd
		}
	};
	cmd.stdin(Stdio::null());
	match cmd.spawn() {
		Ok(child) => Some(LogFollower { child }),
		Err(err) => {
			warn!(%err, "could not start log follower; continuing without it");
			None
		}
	}
}

/// What `plan_stop` decided to do for the Down-side normalisation phase.
///
/// Three lists because the two supervisors normalise to "absent" via
/// different verbs:
///
/// - systemd: `stop` for running units (keep `disable` empty if the unit
///   was already disabled), `disable` for enabled units (keep `stop` empty
///   if the unit was already inactive), or both.
/// - pm2: `delete` for any registered process — pm2 has no "stop but stay
///   registered" + "disable from list" split, so the only way to clear a
///   Down expectation is to unregister entirely.
#[derive(Default, Debug)]
struct StopPlan {
	/// systemd units to `systemctl stop`. Empty on pm2.
	stop: Vec<String>,
	/// systemd units to `systemctl disable`. Empty on pm2.
	disable: Vec<String>,
	/// pm2 process names to `pm2 delete`. Empty on systemd.
	delete: Vec<String>,
}

impl StopPlan {
	fn is_empty(&self) -> bool {
		self.stop.is_empty() && self.disable.is_empty() && self.delete.is_empty()
	}
}

/// Compute the stop+disable list for every `Down` expectation in `groups`.
///
/// - Running discovered instances get added to `stop`.
/// - Enabled units (whether discovered or not — we also probe each
///   expectation's required units to catch the `enabled-but-not-loaded`
///   edge case) get added to `disable`.
///
/// The `is_enabled` probe is taken as a parameter so the test suite can
/// drive the planner without shelling out to `systemctl`.
fn plan_stop(
	supervisor: Supervisor,
	groups: &[(&Expectation, Vec<Instance>)],
	is_enabled: impl Fn(&str) -> bool,
) -> StopPlan {
	let mut plan = StopPlan::default();
	for (exp, instances) in groups {
		if exp.state != ExpectedState::Down {
			continue;
		}
		match supervisor {
			Supervisor::Systemd => {
				for inst in instances {
					if inst.running {
						plan.stop.push(inst.unit());
					}
				}
				// Probe is-enabled for every required unit and any
				// discovered instance, deduping. Catches both the
				// stopped-but-enabled case (discovered, not running, will
				// auto-start) and the enabled-but-not-loaded case (not in
				// `list-units` output but `is-enabled` says yes).
				let mut to_probe: Vec<String> = exp.instances.required_systemd_units(exp.name);
				for inst in instances {
					let u = inst.unit();
					if !to_probe.contains(&u) {
						to_probe.push(u);
					}
				}
				for unit in to_probe {
					if is_enabled(&unit) {
						plan.disable.push(unit);
					}
				}
			}
			Supervisor::Pm2 => {
				// `pm2 delete` is sufficient on its own: it stops the process
				// if running, then unregisters it. Dedupe by name because pm2
				// clustering produces multiple entries sharing one name and
				// `pm2 delete <name>` removes them all in a single call.
				for inst in instances {
					if !plan.delete.contains(&inst.name) {
						plan.delete.push(inst.name.clone());
					}
				}
			}
		}
	}
	plan
}

async fn execute_stop(supervisor: Supervisor, plan: &StopPlan) -> Result<()> {
	if !plan.stop.is_empty() {
		tracing::info!(targets = ?plan.stop, "stopping services expected down");
		lifecycle::stop_targets(supervisor, &plan.stop).await?;
		lifecycle::wait_stopped(supervisor, &plan.stop).await?;
	}
	if !plan.disable.is_empty() {
		tracing::info!(units = ?plan.disable, "disabling units expected down");
		lifecycle::disable_systemd_units(&plan.disable).await?;
	}
	if !plan.delete.is_empty() {
		tracing::info!(processes = ?plan.delete, "deleting pm2 processes expected down");
		lifecycle::delete_pm2(&plan.delete)?;
	}
	Ok(())
}

/// What `plan_start` decided to do.
struct Plan {
	targets: Vec<String>,
	/// Whether any of the planned starts was for a `behind_caddy: true`
	/// expectation — drives the post-start caddy reload so Caddy sees the
	/// fresh container IPs of the services that just came up.
	started_behind_caddy: bool,
}

/// Compute the list of supervisor identifiers to start.
///
/// For systemd: missing units from `required_systemd_units`, plus any
/// known-stopped units within the expectation's admitted set.
/// For pm2: known-stopped processes. Bails if the expectation needs
/// more instances than pm2 has registered.
fn plan_start(
	supervisor: Supervisor,
	groups: &[(&Expectation, Vec<Instance>)],
) -> Result<Plan> {
	let mut targets = Vec::new();
	let mut started_behind_caddy = false;
	for (exp, instances) in groups {
		if exp.state != ExpectedState::Up {
			continue;
		}
		let before = targets.len();
		match supervisor {
			Supervisor::Systemd => {
				let required = exp.instances.required_systemd_units(exp.name);
				let running: HashSet<String> =
					instances.iter().filter(|i| i.running).map(Instance::unit).collect();
				for unit in required {
					if !running.contains(&unit) {
						targets.push(unit);
					}
				}
			}
			Supervisor::Pm2 => {
				let registered = instances.len();
				let needed = exp.instances.min_count();
				if registered < needed {
					bail!(
						"`{}` needs at least {needed} pm2 process(es) but only {registered} are \
						 registered. First-time pm2 registration is the ops setup playbook's \
						 job; tamanu start won't add new entries to the ecosystem.",
						exp.name,
					);
				}
				for inst in instances {
					if !inst.running {
						targets.push(inst.name.clone());
					}
				}
			}
		}
		if targets.len() > before && exp.behind_caddy {
			started_behind_caddy = true;
		}
	}
	Ok(Plan {
		targets,
		started_behind_caddy,
	})
}

async fn systemctl_start(units: &[String]) -> Result<()> {
	systemd::start(units).await
}

fn pm2_start(names: &[String]) -> Result<()> {
	let status = std::process::Command::new("pm2")
		.arg("start")
		.args(names)
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("pm2 start failed: exit {status}");
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use bestool_tamanu::services::Instances;

	fn exp(name: &'static str, behind_caddy: bool) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Up,
			reason: "test".into(),
			legacy: false,
			behind_caddy,
		}
	}

	fn unknown_exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Unknown,
			reason: "test: DB unreachable".into(),
			legacy: false,
			behind_caddy: true,
		}
	}

	#[test]
	fn plan_start_skips_unknown_expectations() {
		// Unknown means "we don't know what this should be" — start must
		// not touch it.
		let portal = unknown_exp("tamanu-patientportal");
		let groups = vec![(&portal, Vec::<Instance>::new())];
		let plan = plan_start(Supervisor::Systemd, &groups).unwrap();
		assert!(plan.targets.is_empty(), "Unknown must not be started");
		assert!(!plan.started_behind_caddy);
	}

	#[test]
	fn plan_stop_skips_unknown_expectations() {
		// Same on the stop side: even if a discovered instance is running,
		// Unknown means hands-off.
		let portal = unknown_exp("tamanu-patientportal");
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&portal,
			vec![Instance {
				name: "tamanu-patientportal".into(),
				instance: None,
				pm_id: None,
				running: true,
			}],
		)];
		let plan = plan_stop(Supervisor::Systemd, &groups, |unit| {
			panic!("is_enabled must not be called for Unknown, got {unit}");
		});
		assert!(plan.is_empty(), "Unknown must not be stopped or disabled");
	}

	#[test]
	fn started_behind_caddy_set_when_a_behind_caddy_unit_is_planned() {
		let api = exp("tamanu-central-api", true);
		let groups = vec![(&api, Vec::<Instance>::new())];
		let plan = plan_start(Supervisor::Systemd, &groups).unwrap();
		assert!(!plan.targets.is_empty());
		assert!(plan.started_behind_caddy);
	}

	#[test]
	fn started_behind_caddy_unset_when_only_internal_planned() {
		// Internal-only services (tasks, workers) trigger no caddy reload —
		// caddy doesn't route to them.
		let tasks = exp("tamanu-central-tasks", false);
		let groups = vec![(&tasks, Vec::<Instance>::new())];
		let plan = plan_start(Supervisor::Systemd, &groups).unwrap();
		assert!(!plan.targets.is_empty());
		assert!(!plan.started_behind_caddy);
	}

	#[test]
	fn started_behind_caddy_unset_when_behind_caddy_already_running() {
		// behind-caddy expectation is fully satisfied — no targets, no caddy reload.
		let api = exp("tamanu-central-api", true);
		let already_running = vec![Instance {
			name: "tamanu-central-api".into(),
			instance: None,
			pm_id: None,
			running: true,
		}];
		let groups = vec![(&api, already_running)];
		let plan = plan_start(Supervisor::Systemd, &groups).unwrap();
		assert!(plan.targets.is_empty());
		assert!(!plan.started_behind_caddy);
	}

	#[test]
	fn started_behind_caddy_tracks_any_behind_caddy_in_a_mixed_batch() {
		let tasks = exp("tamanu-central-tasks", false);
		let api = exp("tamanu-central-api", true);
		let groups = vec![
			(&tasks, Vec::<Instance>::new()),
			(&api, Vec::<Instance>::new()),
		];
		let plan = plan_start(Supervisor::Systemd, &groups).unwrap();
		assert!(plan.started_behind_caddy);
	}

	fn down_exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Down,
			reason: "test".into(),
			legacy: false,
			behind_caddy: false,
		}
	}

	#[test]
	fn plan_stop_collects_running_down_instances() {
		// Running Down unit → systemd stop list, plus disable if enabled.
		let portal = down_exp("tamanu-patientportal");
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&portal,
			vec![Instance {
				name: "tamanu-patientportal".into(),
				instance: None,
				pm_id: None,
				running: true,
			}],
		)];
		let plan = plan_stop(Supervisor::Systemd, &groups, |_| true);
		assert_eq!(plan.stop, vec!["tamanu-patientportal.service"]);
		assert_eq!(plan.disable, vec!["tamanu-patientportal.service"]);
	}

	#[test]
	fn plan_stop_disables_stopped_but_enabled_down_unit() {
		// Not running but `is_enabled` says yes → no stop call (already
		// stopped) but still disable it so it doesn't come back at boot.
		let portal = down_exp("tamanu-patientportal");
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&portal,
			vec![Instance {
				name: "tamanu-patientportal".into(),
				instance: None,
				pm_id: None,
				running: false,
			}],
		)];
		let plan = plan_stop(Supervisor::Systemd, &groups, |_| true);
		assert!(plan.stop.is_empty());
		assert_eq!(plan.disable, vec!["tamanu-patientportal.service"]);
	}

	#[test]
	fn plan_stop_handles_enabled_but_not_loaded_down_unit() {
		// No matching instances at all, but `is_enabled` says yes — we still
		// want to disable so the unit doesn't sneak back at next boot.
		let portal = down_exp("tamanu-patientportal");
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(&portal, vec![])];
		let plan = plan_stop(Supervisor::Systemd, &groups, |_| true);
		assert!(plan.stop.is_empty());
		assert_eq!(plan.disable, vec!["tamanu-patientportal.service"]);
	}

	#[test]
	fn plan_stop_noop_when_down_unit_fully_absent() {
		// Already in the expected state: not running, not enabled, not
		// loaded. plan_stop should produce nothing.
		let portal = down_exp("tamanu-patientportal");
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(&portal, vec![])];
		let plan = plan_stop(Supervisor::Systemd, &groups, |_| false);
		assert!(plan.is_empty());
	}

	#[test]
	fn plan_stop_ignores_up_expectations() {
		// An Up expectation with a (transiently) stopped instance must not
		// be added to the stop/disable list — the start phase will bring it
		// back, not normalise it away.
		let api = exp("tamanu-central-api", true);
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&api,
			vec![Instance {
				name: "tamanu-central-api".into(),
				instance: None,
				pm_id: None,
				running: false,
			}],
		)];
		let plan = plan_stop(Supervisor::Systemd, &groups, |unit| {
			panic!("is_enabled probe must not fire for Up expectations, got {unit}")
		});
		assert!(plan.is_empty());
	}

	#[test]
	fn plan_stop_pm2_deletes_registered_down_regardless_of_run_state() {
		// pm2 has no plain disable; the only way to clear a Down expectation
		// is to delete (unregister) the process. Running or stopped doesn't
		// matter — both register as "present" in pm2's list and would still
		// be flagged by `tamanu status`.
		let fhir = down_exp("tamanu-fhir-resolve");
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&fhir,
			vec![Instance {
				name: "tamanu-fhir-resolve".into(),
				instance: None,
				pm_id: Some(3),
				running: false,
			}],
		)];
		let plan = plan_stop(Supervisor::Pm2, &groups, |unit| {
			panic!("is_enabled probe is meaningless on pm2, got {unit}")
		});
		assert!(plan.stop.is_empty());
		assert!(plan.disable.is_empty());
		assert_eq!(plan.delete, vec!["tamanu-fhir-resolve"]);
	}

	#[test]
	fn plan_stop_pm2_dedupes_cluster_instances_by_name() {
		// Pm2 clustering registers N processes sharing one name.
		// `pm2 delete <name>` removes all of them in a single call, so the
		// plan should only carry the name once even when discovery returns
		// multiple entries.
		let fhir = down_exp("tamanu-fhir-resolve");
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&fhir,
			vec![
				Instance {
					name: "tamanu-fhir-resolve".into(),
					instance: None,
					pm_id: Some(3),
					running: true,
				},
				Instance {
					name: "tamanu-fhir-resolve".into(),
					instance: None,
					pm_id: Some(4),
					running: true,
				},
			],
		)];
		let plan = plan_stop(Supervisor::Pm2, &groups, |_| false);
		assert_eq!(plan.delete, vec!["tamanu-fhir-resolve"]);
	}

	fn down_behind_caddy_exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Down,
			reason: "test".into(),
			legacy: false,
			behind_caddy: true,
		}
	}

	fn inst(name: &str, running: bool) -> Instance {
		Instance {
			name: name.into(),
			instance: None,
			pm_id: None,
			running,
		}
	}

	#[test]
	fn instances_to_probe_includes_behind_caddy_running_up() {
		let api = exp("tamanu-central-api", true);
		let groups = vec![(&api, vec![inst("tamanu-central-api", true)])];
		let probed = instances_to_probe(&groups);
		assert_eq!(probed.len(), 1);
		assert_eq!(probed[0].name, "tamanu-central-api");
	}

	#[test]
	fn instances_to_probe_excludes_non_behind_caddy() {
		let tasks = exp("tamanu-central-tasks", false);
		let groups = vec![(&tasks, vec![inst("tamanu-central-tasks", true)])];
		assert!(instances_to_probe(&groups).is_empty());
	}

	#[test]
	fn instances_to_probe_excludes_not_running() {
		let api = exp("tamanu-central-api", true);
		let groups = vec![(&api, vec![inst("tamanu-central-api", false)])];
		assert!(instances_to_probe(&groups).is_empty());
	}

	#[test]
	fn instances_to_probe_excludes_non_up() {
		// Down (and Unknown) behind-caddy services must not be probed — we
		// didn't start them, so their readiness is none of start's business.
		let portal = down_behind_caddy_exp("tamanu-patientportal");
		let unknown = unknown_exp("tamanu-patientportal-2");
		let groups = vec![
			(&portal, vec![inst("tamanu-patientportal", true)]),
			(&unknown, vec![inst("tamanu-patientportal-2", true)]),
		];
		assert!(instances_to_probe(&groups).is_empty());
	}

	#[test]
	fn instances_to_probe_mixed_batch_keeps_only_eligible() {
		let api = exp("tamanu-central-api", true);
		let tasks = exp("tamanu-central-tasks", false);
		let portal = down_behind_caddy_exp("tamanu-patientportal");
		let groups = vec![
			(&api, vec![inst("tamanu-central-api", true)]),
			(&tasks, vec![inst("tamanu-central-tasks", true)]),
			(&portal, vec![inst("tamanu-patientportal", true)]),
		];
		let probed = instances_to_probe(&groups);
		assert_eq!(probed.len(), 1);
		assert_eq!(probed[0].name, "tamanu-central-api");
	}
}
