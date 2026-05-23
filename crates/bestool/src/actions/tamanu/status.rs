use clap::Parser;
use miette::{IntoDiagnostic, Result, bail};
use owo_colors::OwoColorize;
use serde::Serialize;

use crate::actions::{
	Context,
	tamanu::{
		TamanuArgs,
		lifecycle::{self, Instance},
		services::{self, Criticality, ExpectedState, Expectation},
	},
};

/// Report on tamanu services: what's expected vs what's actually running.
///
/// A lighter cousin of `tamanu doctor`: discovery only, no HTTP probes or
/// database queries. Useful as a quick "is anything down right now?"
/// check, or before/after a `tamanu start` / `restart` to see the impact.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct StatusArgs {
	/// Limit to expectations whose name contains any of these substrings.
	/// No names = report on every expectation.
	pub names: Vec<String>,

	/// Emit the wire-shape JSON instead of the human-readable render.
	#[arg(long)]
	pub json: bool,
}

pub async fn run(args: StatusArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();
	let use_colours = tamanu.use_colours;

	let (supervisor, expectations) = lifecycle::config_and_expectations(tamanu)?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = services::match_names(&expectations, &names)?;
	let discovered = lifecycle::discover(supervisor)?;
	let groups = lifecycle::group_by_expectation(&matched, &discovered);

	if args.json {
		let report = build_report(&groups);
		println!(
			"{}",
			serde_json::to_string_pretty(&report).into_diagnostic()?
		);
		if report.any_short {
			bail!("some expectations are not met");
		}
		return Ok(());
	}

	let mut any_short = false;
	for (exp, instances) in &groups {
		let short = render(exp, instances, use_colours);
		if short {
			any_short = true;
		}
	}

	if any_short {
		bail!("some expectations are not met");
	}
	Ok(())
}

#[derive(Serialize)]
struct Report {
	any_short: bool,
	expectations: Vec<ExpectationReport>,
}

#[derive(Serialize)]
struct ExpectationReport {
	name: &'static str,
	expected_state: &'static str,
	criticality: &'static str,
	running: usize,
	min_count: usize,
	status: &'static str,
	instances: Vec<InstanceReport>,
}

#[derive(Serialize)]
struct InstanceReport {
	name: String,
	instance: Option<String>,
	pm_id: Option<i64>,
	running: bool,
}

fn build_report(groups: &[(&Expectation, Vec<Instance>)]) -> Report {
	let mut any_short = false;
	let expectations = groups
		.iter()
		.map(|(exp, instances)| {
			let running = instances.iter().filter(|i| i.running).count();
			let status = match exp.state {
				ExpectedState::Up if running >= exp.instances.min_count() => "up",
				ExpectedState::Up if running == 0 => {
					any_short = true;
					"down"
				}
				ExpectedState::Up => {
					any_short = true;
					"short"
				}
				ExpectedState::Down if instances.is_empty() => "absent",
				ExpectedState::Down => {
					any_short = true;
					"forbidden"
				}
			};
			ExpectationReport {
				name: exp.name,
				expected_state: match exp.state {
					ExpectedState::Up => "up",
					ExpectedState::Down => "down",
				},
				criticality: match exp.criticality {
					Criticality::Critical => "critical",
					Criticality::Background => "background",
				},
				running,
				min_count: exp.instances.min_count(),
				status,
				instances: instances
					.iter()
					.map(|i| InstanceReport {
						name: i.name.clone(),
						instance: i.instance.clone(),
						pm_id: i.pm_id,
						running: i.running,
					})
					.collect(),
			}
		})
		.collect();
	Report {
		any_short,
		expectations,
	}
}

/// Render one expectation + its discovered instances. Returns true if
/// the expectation is "short": Up but with fewer running instances than
/// `min_count`, or Down but with anything present.
fn render(exp: &Expectation, instances: &[Instance], use_colours: bool) -> bool {
	let running = instances.iter().filter(|i| i.running).count();

	let (label, short) = match exp.state {
		ExpectedState::Up => {
			let needed = exp.instances.min_count();
			if running >= needed {
				(painted("UP", "green", use_colours), false)
			} else if running == 0 {
				(painted("DOWN", "red", use_colours), true)
			} else {
				(painted("SHORT", "yellow", use_colours), true)
			}
		}
		ExpectedState::Down => {
			if instances.is_empty() {
				(painted("ABSENT", "green", use_colours), false)
			} else {
				(painted("FORBIDDEN", "red", use_colours), true)
			}
		}
	};

	let crit = match (exp.state, exp.criticality) {
		(ExpectedState::Up, Criticality::Critical) => " (critical)",
		_ => "",
	};

	let needed = match exp.state {
		ExpectedState::Up => format!("{}/{}", running, exp.instances.min_count()),
		ExpectedState::Down => format!("{}", instances.len()),
	};

	println!("{:10} {} [{}]{}", label, exp.name, needed, crit);
	for inst in instances {
		let status = if inst.running {
			painted("running", "green", use_colours)
		} else {
			painted("stopped", "yellow", use_colours)
		};
		println!("           {} {}", inst.display(), status);
	}

	short
}

fn painted(s: &str, colour: &str, on: bool) -> String {
	if !on {
		return s.to_string();
	}
	match colour {
		"green" => s.green().to_string(),
		"red" => s.red().to_string(),
		"yellow" => s.yellow().to_string(),
		_ => s.to_string(),
	}
}
