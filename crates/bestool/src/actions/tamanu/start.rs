use std::collections::HashSet;

use clap::Parser;
use miette::{IntoDiagnostic, Result, bail};

use bestool_tamanu::services::{
	self, Criticality, ExpectedState, Expectation, Supervisor, systemd_is_enabled,
};

use crate::actions::{
	Context,
	tamanu::{
		TamanuArgs,
		lifecycle::{self, Instance},
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
}

pub async fn run(args: StartArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();

	let (supervisor, expectations) = lifecycle::config_and_expectations(tamanu).await?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = services::match_names(&expectations, &names)?;
	let discovered = lifecycle::discover(supervisor)?;
	let groups = lifecycle::group_by_expectation(&matched, &discovered);

	let stop_plan = if args.up_only {
		StopPlan::default()
	} else {
		plan_stop(supervisor, &groups, systemd_is_enabled)
	};
	let Plan {
		targets,
		started_critical,
	} = plan_start(supervisor, &groups)?;
	if stop_plan.is_empty() && targets.is_empty() {
		tracing::info!("nothing to do; everything matches expected state");
		return Ok(());
	}

	lifecycle::ensure_root_or_reexec(supervisor)?;

	if !stop_plan.is_empty() {
		execute_stop(supervisor, &stop_plan)?;
	}

	if !targets.is_empty() {
		tracing::info!(?targets, "starting");
		match supervisor {
			Supervisor::Systemd => systemctl_start(&targets)?,
			Supervisor::Pm2 => pm2_start(&targets)?,
		}
		lifecycle::wait_running(supervisor, &targets)?;
	}

	// Critical services are the API and frontend, whose containers Caddy
	// reaches by hostname. When one of those comes up fresh, Caddy's
	// upstream DNS cache may still hold a stale (NXDOMAIN) entry from
	// before the container existed — `restart` handles this per-instance,
	// but `start` brings up a batch in one go, so a single reload at the
	// end covers all of them.
	if started_critical && matches!(supervisor, Supervisor::Systemd) {
		lifecycle::reload_caddy();
	}

	Ok(())
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

fn execute_stop(supervisor: Supervisor, plan: &StopPlan) -> Result<()> {
	if !plan.stop.is_empty() {
		tracing::info!(targets = ?plan.stop, "stopping services expected down");
		lifecycle::stop_targets(supervisor, &plan.stop)?;
		lifecycle::wait_stopped(supervisor, &plan.stop)?;
	}
	if !plan.disable.is_empty() {
		tracing::info!(units = ?plan.disable, "disabling units expected down");
		lifecycle::disable_systemd_units(&plan.disable)?;
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
	/// Whether any of the planned starts was for a `Criticality::Critical`
	/// expectation — drives the post-start caddy reload.
	started_critical: bool,
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
	let mut started_critical = false;
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
		if targets.len() > before && exp.criticality == Criticality::Critical {
			started_critical = true;
		}
	}
	Ok(Plan {
		targets,
		started_critical,
	})
}

fn systemctl_start(units: &[String]) -> Result<()> {
	let status = std::process::Command::new("systemctl")
		.arg("start")
		.args(units)
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("systemctl start failed: exit {status}");
	}
	Ok(())
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

	fn exp(name: &'static str, crit: Criticality) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Up,
			criticality: crit,
			reason: "test".into(),
			legacy: false,
		}
	}

	#[test]
	fn started_critical_set_when_a_critical_unit_is_planned() {
		let api = exp("tamanu-central-api", Criticality::Critical);
		let groups = vec![(&api, Vec::<Instance>::new())];
		let plan = plan_start(Supervisor::Systemd, &groups).unwrap();
		assert!(!plan.targets.is_empty());
		assert!(plan.started_critical);
	}

	#[test]
	fn started_critical_unset_when_only_background_planned() {
		let tasks = exp("tamanu-central-tasks", Criticality::Background);
		let groups = vec![(&tasks, Vec::<Instance>::new())];
		let plan = plan_start(Supervisor::Systemd, &groups).unwrap();
		assert!(!plan.targets.is_empty());
		assert!(!plan.started_critical);
	}

	#[test]
	fn started_critical_unset_when_critical_already_running() {
		// Critical expectation is fully satisfied — no targets, no caddy reload.
		let api = exp("tamanu-central-api", Criticality::Critical);
		let already_running = vec![Instance {
			name: "tamanu-central-api".into(),
			instance: None,
			pm_id: None,
			running: true,
		}];
		let groups = vec![(&api, already_running)];
		let plan = plan_start(Supervisor::Systemd, &groups).unwrap();
		assert!(plan.targets.is_empty());
		assert!(!plan.started_critical);
	}

	#[test]
	fn started_critical_tracks_any_critical_in_a_mixed_batch() {
		let tasks = exp("tamanu-central-tasks", Criticality::Background);
		let api = exp("tamanu-central-api", Criticality::Critical);
		let groups = vec![
			(&tasks, Vec::<Instance>::new()),
			(&api, Vec::<Instance>::new()),
		];
		let plan = plan_start(Supervisor::Systemd, &groups).unwrap();
		assert!(plan.started_critical);
	}

	fn down_exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Down,
			criticality: Criticality::Background,
			reason: "test".into(),
			legacy: false,
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
		let api = exp("tamanu-central-api", Criticality::Critical);
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
}
