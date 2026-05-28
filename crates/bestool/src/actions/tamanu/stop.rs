use clap::Parser;
use miette::Result;

use bestool_tamanu::services::{self, ExpectedState, Expectation, Supervisor};

use crate::actions::{
	Context,
	tamanu::{
		TamanuArgs,
		lifecycle::{self, Instance},
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

	let (supervisor, expectations) = lifecycle::config_and_expectations(tamanu).await?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = services::match_names(&expectations, &names)?;
	let discovered = lifecycle::discover(supervisor)?;
	let groups = lifecycle::group_by_expectation(&matched, &discovered);

	let targets = plan_stop(supervisor, &groups);
	if targets.is_empty() {
		tracing::info!("nothing to stop; everything matched is already down");
		return Ok(());
	}

	lifecycle::ensure_root_or_reexec(supervisor)?;

	tracing::info!(?targets, "stopping");
	lifecycle::stop_targets(supervisor, &targets)?;
	lifecycle::wait_stopped(supervisor, &targets).await?;
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

