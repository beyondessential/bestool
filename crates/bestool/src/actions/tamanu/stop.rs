use clap::Parser;
use miette::{IntoDiagnostic, Result, bail};

use crate::actions::{
	Context,
	tamanu::{
		TamanuArgs,
		lifecycle::{self, Instance},
		services::{ExpectedState, Expectation, Supervisor},
	},
};

/// Stop running tamanu services.
///
/// All matched services are stopped in a single supervisor call. Caddy
/// is not touched: its upstreams just become unreachable, which is
/// usually what's intended for a maintenance window.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct StopArgs {
	/// Limit to expectations whose name contains any of these substrings.
	/// No names = stop every running instance of every Up expectation.
	pub names: Vec<String>,
}

pub async fn run(args: StopArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();

	let (supervisor, expectations) = lifecycle::config_and_expectations(tamanu)?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = lifecycle::match_names(&expectations, &names)?;
	let discovered = lifecycle::discover(supervisor)?;
	let groups = lifecycle::group_by_expectation(&matched, &discovered);

	let targets = plan_stop(supervisor, &groups);
	if targets.is_empty() {
		tracing::info!("nothing to stop; everything matched is already down");
		return Ok(());
	}

	lifecycle::ensure_root_or_reexec(supervisor)?;

	tracing::info!(?targets, "stopping");
	match supervisor {
		Supervisor::Systemd => systemctl_stop(&targets)?,
		Supervisor::Pm2 => pm2_stop(&targets)?,
	}

	lifecycle::wait_stopped(supervisor, &targets)?;
	Ok(())
}

fn plan_stop(
	supervisor: Supervisor,
	groups: &[(&Expectation, Vec<Instance>)],
) -> Vec<String> {
	let mut targets = Vec::new();
	for (exp, instances) in groups {
		if exp.state != ExpectedState::Up {
			continue;
		}
		for inst in instances {
			if !inst.running {
				continue;
			}
			let id = match supervisor {
				Supervisor::Systemd => inst.unit(),
				Supervisor::Pm2 => inst.name.clone(),
			};
			targets.push(id);
		}
	}
	targets
}

fn systemctl_stop(units: &[String]) -> Result<()> {
	let status = std::process::Command::new("systemctl")
		.arg("stop")
		.args(units)
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("systemctl stop failed: exit {status}");
	}
	Ok(())
}

fn pm2_stop(names: &[String]) -> Result<()> {
	let status = std::process::Command::new("pm2")
		.arg("stop")
		.args(names)
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("pm2 stop failed: exit {status}");
	}
	Ok(())
}
