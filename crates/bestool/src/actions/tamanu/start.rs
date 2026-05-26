use std::collections::HashSet;

use clap::Parser;
use miette::{IntoDiagnostic, Result, bail};

use bestool_tamanu::services::{self, Criticality, ExpectedState, Expectation, Supervisor};

use crate::actions::{
	Context,
	tamanu::{
		TamanuArgs,
		lifecycle::{self, Instance},
	},
};

/// Bring up any expected tamanu services that aren't running.
///
/// Idempotent: services already up are left alone. Use `tamanu status`
/// first if you want to see what's missing.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct StartArgs {
	/// Limit to expectations whose name contains any of these substrings.
	/// No names = start every Up expectation that's currently short.
	pub names: Vec<String>,
}

pub async fn run(args: StartArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();

	let (supervisor, expectations) = lifecycle::config_and_expectations(tamanu).await?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = services::match_names(&expectations, &names)?;
	let discovered = lifecycle::discover(supervisor)?;
	let groups = lifecycle::group_by_expectation(&matched, &discovered);

	let Plan {
		targets,
		started_critical,
	} = plan_start(supervisor, &groups)?;
	if targets.is_empty() {
		tracing::info!("nothing to start; everything expected is already up");
		return Ok(());
	}

	lifecycle::ensure_root_or_reexec(supervisor)?;

	tracing::info!(?targets, "starting");
	match supervisor {
		Supervisor::Systemd => systemctl_start(&targets)?,
		Supervisor::Pm2 => pm2_start(&targets)?,
	}

	lifecycle::wait_running(supervisor, &targets)?;

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
}
