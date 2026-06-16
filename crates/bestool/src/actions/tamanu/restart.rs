use std::{collections::HashSet, time::Duration};

use clap::Parser;
use miette::Result;
use reqwest::{Client, Url};
use tracing::{debug, info, warn};

use bestool_tamanu::{
	services::{self, ExpectedState, Expectation, Supervisor, parse_systemd_unit},
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

/// Rolling-restart all running tamanu services.
///
/// Background services (tasks, sync, fhir-*) restart in a single bulk
/// supervisor call. Critical services (api, frontend) restart one
/// instance at a time, each followed by a readiness probe, caddy
/// reload, and a cooldown — so there's always at least one critical
/// instance up to take traffic.
///
/// Services expected up but not currently running are started first,
/// before any restarts, so capacity is back at full strength before
/// the roll begins.
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
	#[arg(long, default_value = "30s", value_parser = probe::parse_duration)]
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

pub async fn run(args: RestartArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();

	let (supervisor, expectations) =
		lifecycle::config_and_expectations(tamanu, WaitForDb::No).await?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = services::match_names(&expectations, &names)?;
	lifecycle::warn_unknown_expectations(&matched);
	let discovered = lifecycle::discover(supervisor).await?;
	let groups = lifecycle::group_by_expectation(supervisor, &matched, &discovered);
	// Leftover singletons on a host that's migrated to an instanced layout:
	// retired (stop + disable) at the end, once their instanced replacements
	// are up and serving. Detected against the full discovered set, since
	// `group_by_expectation` no longer admits them into the instanced group.
	let retire = lifecycle::stale_shape_groups(supervisor, &matched, &discovered);

	lifecycle::ensure_root_or_reexec(supervisor)?;

	let Partitioned {
		start,
		bulk,
		batch_behind_caddy,
		rolling,
	} = partition(supervisor, &groups);
	let client = probe::http_client()?;

	if !start.is_empty() {
		info!(targets = ?start, "starting missing services");
		lifecycle::start_targets(supervisor, &start).await?;
		lifecycle::wait_running(supervisor, &start).await?;
	}

	if !bulk.is_empty() {
		info!(targets = ?bulk, "bulk-restarting non-rolling services");
		bulk_restart(supervisor, &bulk).await?;
		lifecycle::wait_running(supervisor, &bulk).await?;
	} else {
		debug!("no bulk-restart services");
	}

	// One reload after the start+bulk batch covers every behind-caddy
	// service in it — relevant for older deployments whose patient-portal
	// is a singleton (the frontend always rolls, and on newer
	// deployments the patient-portal does too). Per-service rolling
	// reloads aren't needed for singletons: bulk-restart already
	// drops them all briefly, so a single trailing reload is enough
	// to flush Caddy's stale upstream IPs.
	if batch_behind_caddy {
		lifecycle::reload_caddy().await;
	}

	for (i, (instance, behind_caddy)) in rolling.iter().enumerate() {
		info!(
			"rolling restart {}/{}: {}",
			i + 1,
			rolling.len(),
			instance.display(),
		);
		lifecycle::restart_one(supervisor, instance).await?;
		lifecycle::wait_running_one(supervisor, instance, Duration::from_secs(60)).await?;

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

		if *behind_caddy {
			lifecycle::reload_caddy().await;
		}

		// Cooldown only applies when we have no readiness signal at all —
		// either probing is disabled (`--no-probe-http`) or we couldn't
		// construct a probe URL. A failed probe is impossible here: the
		// probe loop retries forever until it succeeds.
		if i + 1 < rolling.len() && !probed_ready {
			debug!(seconds = args.cooldown.as_secs(), "cooldown");
			tokio::time::sleep(args.cooldown).await;
		}
	}

	// Retire leftover singletons last: the instanced replacements were
	// brought up in the start batch (and Caddy reloaded), so cutting the
	// singleton now completes a singleton→instanced migration with no gap in
	// service. We confirm the replacements are actually serving first.
	if !retire.is_empty() {
		for (exp, instances) in &retire {
			let stale: Vec<String> = instances.iter().map(Instance::display).collect();
			info!(
				service = exp.name,
				?stale,
				"retiring leftover units after migration to instanced layout"
			);
			if !args.no_probe_http {
				wait_replacements_ready(supervisor, exp, &client).await?;
			} else {
				// No readiness signal to wait on; give the freshly-started
				// replacements the same grace a roll would before cutting over.
				debug!(seconds = args.cooldown.as_secs(), "cooldown before retiring");
				tokio::time::sleep(args.cooldown).await;
			}
		}
		let units: Vec<String> = retire
			.iter()
			.flat_map(|(_, instances)| instances.iter().map(Instance::unit))
			.collect();
		lifecycle::retire_systemd_units(&units).await;
	}

	if let Some(url) = &args.check_url {
		info!(%url, "final end-to-end probe");
		probe::probe_url(&client, url, Duration::from_secs(60)).await?;
	}

	Ok(())
}

/// Block until every instanced unit the expectation requires responds to an
/// HTTP probe. Used before retiring a leftover singleton, so we never cut the
/// old unit until its replacements are serving. Units without a constructable
/// probe URL (no container IP yet) are skipped — same best-effort semantics as
/// the rolling probe.
async fn wait_replacements_ready(
	supervisor: Supervisor,
	exp: &Expectation,
	client: &Client,
) -> Result<()> {
	for unit in exp.instances.required_systemd_units(exp.name) {
		let Some((base, instance)) = parse_systemd_unit(&unit) else {
			continue;
		};
		let inst = Instance {
			name: base.to_string(),
			instance: instance.map(str::to_string),
			pm_id: None,
			running: true,
		};
		if let Some(url) = probe::instance_probe_url(supervisor, &inst)? {
			probe::probe_until_ready(client, &url).await;
		}
	}
	Ok(())
}

/// Output of [`partition`]: expected-Up services that aren't running get
/// started; rolling-eligible running services restart one instance at a
/// time with a per-instance readiness probe between each; everything else
/// bulk-restarts. `Down` expectations are dropped entirely (they wouldn't
/// be running, so there's nothing to restart).
///
/// "Rolling-eligible" comes from [`Expectation::rolling_restart`]:
/// `min_count >= 2`, i.e. the expectation has enough instances that
/// rolling can keep one available while the next swaps. Singletons bulk-
/// restart because there's no second instance to take traffic during the
/// roll.
struct Partitioned {
	/// Supervisor-native identifiers to start: expected-Up units that
	/// aren't currently running, whether discovered-but-stopped or not
	/// loaded at all. Same selection as `tamanu start`'s planner — on
	/// systemd, the expectation's required units minus the running ones;
	/// on pm2, registered-but-stopped processes (pm2 can't create
	/// entries, so under-registration only warns).
	start: Vec<String>,
	/// Supervisor-native identifiers to bulk-restart.
	bulk: Vec<String>,
	/// True if any start or bulk entry's expectation has `behind_caddy:
	/// true` — drives a single trailing `reload_caddy` after the batch
	/// completes so Caddy sees the new container IPs (relevant on older
	/// deployments where the patient-portal is still a singleton).
	batch_behind_caddy: bool,
	/// Instances to roll one-at-a-time, paired with their expectation's
	/// `behind_caddy` flag so each iteration knows whether to reload Caddy
	/// after the restart settles.
	rolling: Vec<(Instance, bool)>,
}

fn partition(supervisor: Supervisor, groups: &[(&Expectation, Vec<Instance>)]) -> Partitioned {
	let mut start = Vec::new();
	let mut bulk = Vec::new();
	let mut batch_behind_caddy = false;
	let mut rolling = Vec::new();
	for (exp, instances) in groups {
		if exp.state != ExpectedState::Up {
			continue;
		}
		let start_before = start.len();
		match supervisor {
			Supervisor::Systemd => {
				let running: HashSet<String> = instances
					.iter()
					.filter(|i| i.running)
					.map(Instance::unit)
					.collect();
				for unit in exp.instances.required_systemd_units(exp.name) {
					if !running.contains(&unit) {
						start.push(unit);
					}
				}
			}
			Supervisor::Pm2 => {
				// Unlike `tamanu start`, don't bail on under-registration:
				// restart's primary job is restarting what exists, and
				// failing here would also skip that.
				let registered = instances.len();
				let needed = exp.instances.min_count();
				if registered < needed {
					warn!(
						"`{}` needs at least {needed} pm2 process(es) but only {registered} are \
						 registered; restart can't add new entries — that's the ops setup \
						 playbook's job",
						exp.name,
					);
				}
				for inst in instances {
					if !inst.running {
						start.push(inst.name.clone());
					}
				}
			}
		}
		if start.len() > start_before && exp.behind_caddy {
			batch_behind_caddy = true;
		}
		for inst in instances {
			if !inst.running {
				continue;
			}
			if exp.rolling_restart() {
				rolling.push((inst.clone(), exp.behind_caddy));
			} else {
				bulk.push(match supervisor {
					Supervisor::Systemd => inst.unit(),
					Supervisor::Pm2 => inst.name.clone(),
				});
				if exp.behind_caddy {
					batch_behind_caddy = true;
				}
			}
		}
	}
	Partitioned {
		start,
		bulk,
		batch_behind_caddy,
		rolling,
	}
}

async fn bulk_restart(supervisor: Supervisor, targets: &[String]) -> Result<()> {
	match supervisor {
		Supervisor::Systemd => systemd::restart_all(targets).await,
		Supervisor::Pm2 => lifecycle::pm2_restart_targets(targets),
	}
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
	let Some(url) = probe::instance_probe_url(supervisor, instance)? else {
		return Ok(false);
	};
	probe::probe_until_ready(client, &url).await;
	Ok(true)
}

#[cfg(test)]
mod tests {
	use super::*;
	use bestool_tamanu::services::Instances;

	fn up_exp(name: &'static str, instances: Instances, behind_caddy: bool) -> Expectation {
		Expectation {
			name,
			instances,
			state: ExpectedState::Up,
			reason: "test".into(),
			legacy: false,
			behind_caddy,
		}
	}

	fn inst(name: &str, instance: Option<&str>, running: bool) -> Instance {
		Instance {
			name: name.into(),
			instance: instance.map(Into::into),
			pm_id: None,
			running,
		}
	}

	#[test]
	fn partition_rolls_patient_portal_a_b() {
		// Patient portal is multi-instance (@a/@b) and behind Caddy — like
		// the frontend, it should roll one instance at a time so there's
		// always one up to take traffic.
		let portal = up_exp("tamanu-patientportal", Instances::Named(&["a", "b"]), true);
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&portal,
			vec![
				inst("tamanu-patientportal", Some("a"), true),
				inst("tamanu-patientportal", Some("b"), true),
			],
		)];
		let p = partition(Supervisor::Systemd, &groups);
		assert!(p.start.is_empty());
		assert!(p.bulk.is_empty());
		assert_eq!(p.rolling.len(), 2);
		assert!(p.rolling.iter().all(|(_, behind_caddy)| *behind_caddy));
	}

	#[test]
	fn partition_flags_bulk_behind_caddy_when_singleton_portal_runs() {
		// Older deployments still run patient-portal as a singleton, so a
		// bulk restart must trigger a Caddy reload at the end to flush
		// the stale container IP.
		let portal = up_exp("tamanu-patientportal", Instances::Single, true);
		let tasks = up_exp("tamanu-central-tasks", Instances::Single, false);
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![
			(&portal, vec![inst("tamanu-patientportal", None, true)]),
			(&tasks, vec![inst("tamanu-central-tasks", None, true)]),
		];
		let p = partition(Supervisor::Systemd, &groups);
		assert!(p.batch_behind_caddy);
		assert_eq!(p.bulk.len(), 2);
		assert!(p.rolling.is_empty());
	}

	#[test]
	fn partition_no_bulk_behind_caddy_when_no_caddy_service_in_bulk() {
		// All-internal bulk batch — no caddy reload should fire.
		let tasks = up_exp("tamanu-central-tasks", Instances::Single, false);
		let sync = up_exp("tamanu-facility-sync", Instances::Single, false);
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![
			(&tasks, vec![inst("tamanu-central-tasks", None, true)]),
			(&sync, vec![inst("tamanu-facility-sync", None, true)]),
		];
		let p = partition(Supervisor::Systemd, &groups);
		assert!(!p.batch_behind_caddy);
	}

	#[test]
	fn partition_carries_behind_caddy_per_rolling_instance() {
		// Multi-instance API is rolling-eligible; each rolling entry carries
		// the behind_caddy flag so the loop can reload caddy per swap.
		let api = up_exp("tamanu-central-api", Instances::NumericAtLeast(2), true);
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&api,
			vec![
				inst("tamanu-central-api", Some("1"), true),
				inst("tamanu-central-api", Some("2"), true),
			],
		)];
		let p = partition(Supervisor::Systemd, &groups);
		assert_eq!(p.rolling.len(), 2);
		assert!(p.rolling.iter().all(|(_, behind_caddy)| *behind_caddy));
		assert!(p.bulk.is_empty());
	}

	#[test]
	fn partition_singleton_portal_is_bulk_not_rolling() {
		// Singleton patient-portal (older deployments): rolling needs ≥2
		// instances to keep one up while the other swaps, so a singleton
		// bulk-restarts but still triggers a caddy reload.
		let portal = up_exp("tamanu-patientportal", Instances::Single, true);
		let groups: Vec<(&Expectation, Vec<Instance>)> =
			vec![(&portal, vec![inst("tamanu-patientportal", None, true)])];
		let p = partition(Supervisor::Systemd, &groups);
		assert!(p.rolling.is_empty(), "singleton must not roll");
		assert_eq!(p.bulk.len(), 1);
		assert!(p.batch_behind_caddy);
	}

	#[test]
	fn partition_starts_unit_missing_from_discovery() {
		// Expected-Up singleton with no discovered instances at all — the
		// unit isn't loaded, so restart must start it rather than skip it.
		let tasks = up_exp("tamanu-central-tasks", Instances::Single, false);
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(&tasks, vec![])];
		let p = partition(Supervisor::Systemd, &groups);
		assert_eq!(p.start, vec!["tamanu-central-tasks.service"]);
		assert!(p.bulk.is_empty());
		assert!(p.rolling.is_empty());
		assert!(!p.batch_behind_caddy);
	}

	#[test]
	fn partition_starts_stopped_discovered_instance() {
		// Discovered but inactive: previously silently skipped, now started.
		let tasks = up_exp("tamanu-central-tasks", Instances::Single, false);
		let groups: Vec<(&Expectation, Vec<Instance>)> =
			vec![(&tasks, vec![inst("tamanu-central-tasks", None, false)])];
		let p = partition(Supervisor::Systemd, &groups);
		assert_eq!(p.start, vec!["tamanu-central-tasks.service"]);
		assert!(p.bulk.is_empty());
	}

	#[test]
	fn partition_starts_missing_instance_and_rolls_running_one() {
		// Multi-instance API with @1 running and @2 absent: @2 starts (in
		// the up-front batch, so capacity is back before the roll), @1
		// still rolls.
		let api = up_exp("tamanu-central-api", Instances::NumericAtLeast(2), true);
		let groups: Vec<(&Expectation, Vec<Instance>)> =
			vec![(&api, vec![inst("tamanu-central-api", Some("1"), true)])];
		let p = partition(Supervisor::Systemd, &groups);
		assert_eq!(p.start, vec!["tamanu-central-api@2.service"]);
		assert_eq!(p.rolling.len(), 1);
		assert!(p.batch_behind_caddy, "started behind-caddy unit needs a reload");
	}

	#[test]
	fn partition_does_not_start_down_or_unknown() {
		let down = Expectation {
			name: "tamanu-patientportal",
			instances: Instances::Single,
			state: ExpectedState::Down,
			reason: "test".into(),
			legacy: false,
			behind_caddy: true,
		};
		let unknown = Expectation {
			name: "tamanu-fhir-worker",
			instances: Instances::Single,
			state: ExpectedState::Unknown,
			reason: "test".into(),
			legacy: false,
			behind_caddy: false,
		};
		let groups: Vec<(&Expectation, Vec<Instance>)> =
			vec![(&down, vec![]), (&unknown, vec![])];
		let p = partition(Supervisor::Systemd, &groups);
		assert!(p.start.is_empty());
		assert!(!p.batch_behind_caddy);
	}

	#[test]
	fn partition_pm2_starts_stopped_registered_process() {
		let tasks = up_exp("tamanu-tasks", Instances::Single, false);
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&tasks,
			vec![Instance {
				name: "tamanu-tasks".into(),
				instance: None,
				pm_id: Some(2),
				running: false,
			}],
		)];
		let p = partition(Supervisor::Pm2, &groups);
		assert_eq!(p.start, vec!["tamanu-tasks"]);
		assert!(p.bulk.is_empty());
	}

	#[test]
	fn partition_pm2_under_registration_still_restarts_running() {
		// pm2 can't create new entries, so a short registration only warns;
		// the running process must still be restarted.
		let api = up_exp("tamanu-api", Instances::NumericAtLeast(2), true);
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&api,
			vec![Instance {
				name: "tamanu-api".into(),
				instance: None,
				pm_id: Some(0),
				running: true,
			}],
		)];
		let p = partition(Supervisor::Pm2, &groups);
		assert!(p.start.is_empty());
		assert_eq!(p.rolling.len(), 1);
	}
}
