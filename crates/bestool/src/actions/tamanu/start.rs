use std::collections::HashSet;

use clap::Parser;
use miette::{IntoDiagnostic, Result, bail};

use crate::actions::{
	Context,
	tamanu::{
		TamanuArgs,
		lifecycle::{self, Instance},
		services::{self, ExpectedState, Expectation, Supervisor},
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

	let (supervisor, expectations) = lifecycle::config_and_expectations(tamanu)?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = services::match_names(&expectations, &names)?;
	let discovered = lifecycle::discover(supervisor)?;
	let groups = lifecycle::group_by_expectation(&matched, &discovered);

	let targets = plan_start(supervisor, &groups)?;
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
	Ok(())
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
) -> Result<Vec<String>> {
	let mut targets = Vec::new();
	for (exp, instances) in groups {
		if exp.state != ExpectedState::Up {
			continue;
		}
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
	}
	Ok(targets)
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
