use clap::Parser;
use miette::{IntoDiagnostic, Result, bail};
use owo_colors::OwoColorize;
use serde::Serialize;

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

	let (supervisor, expectations) = lifecycle::config_and_expectations(tamanu).await?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = services::match_names(&expectations, &names)?;
	let discovered = lifecycle::discover(supervisor)?;
	let mut groups = lifecycle::group_by_expectation(&matched, &discovered);
	if matches!(supervisor, Supervisor::Systemd) {
		drop_disabled_down(&mut groups, systemd_is_enabled);
	}

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

/// Filter `Down`-expected groups so that loaded-but-stopped systemd units
/// that are *also* `disabled` are dropped from the visible instance list.
///
/// `list-units --all` reports any unit currently parsed into systemd's
/// memory, including ones that were stopped and disabled but haven't been
/// unloaded yet (typical after a manual `systemctl stop` followed by
/// `systemctl disable`). Such a unit isn't going to start on its own and
/// isn't running now — treating it as "present" against a `Down`
/// expectation produces a false-positive FORBIDDEN. Drop it so the group
/// reads ABSENT, matching how a fresh-rebooted host would see the same
/// state.
fn drop_disabled_down(
	groups: &mut [(&Expectation, Vec<Instance>)],
	is_enabled: impl Fn(&str) -> bool,
) {
	for (exp, instances) in groups.iter_mut() {
		if !matches!(exp.state, ExpectedState::Down) {
			continue;
		}
		instances.retain(|i| i.running || is_enabled(&i.unit()));
	}
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

#[cfg(test)]
mod tests {
	use super::*;
	use bestool_tamanu::services::{Criticality, Instances};

	fn down_exp() -> Expectation {
		Expectation {
			name: "tamanu-patientportal",
			instances: Instances::Single,
			state: ExpectedState::Down,
			criticality: Criticality::Background,
			reason: "test".into(),
		}
	}

	fn up_exp() -> Expectation {
		Expectation {
			name: "tamanu-frontend",
			instances: Instances::Named(&["a", "b"]),
			state: ExpectedState::Up,
			criticality: Criticality::Critical,
			reason: "test".into(),
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
	fn drop_disabled_down_removes_stopped_and_disabled() {
		let exp = down_exp();
		let mut groups: Vec<(&Expectation, Vec<Instance>)> =
			vec![(&exp, vec![inst("tamanu-patientportal", None, false)])];
		drop_disabled_down(&mut groups, |_| false);
		assert!(
			groups[0].1.is_empty(),
			"stopped+disabled Down unit should be dropped",
		);
		let report = build_report(&groups);
		assert!(!report.any_short, "should be ABSENT/OK now");
		assert_eq!(report.expectations[0].status, "absent");
	}

	#[test]
	fn drop_disabled_down_keeps_stopped_but_enabled() {
		let exp = down_exp();
		let mut groups: Vec<(&Expectation, Vec<Instance>)> =
			vec![(&exp, vec![inst("tamanu-patientportal", None, false)])];
		drop_disabled_down(&mut groups, |_| true);
		assert_eq!(groups[0].1.len(), 1);
		let report = build_report(&groups);
		assert!(report.any_short, "stopped+enabled is still FORBIDDEN");
		assert_eq!(report.expectations[0].status, "forbidden");
	}

	#[test]
	fn drop_disabled_down_ignores_up_expectations() {
		// Stopped+disabled instance of an Up expectation must still show up
		// — it's still part of "what's needed but missing".
		let exp = up_exp();
		let mut groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&exp,
			vec![
				inst("tamanu-frontend", Some("a"), true),
				inst("tamanu-frontend", Some("b"), false),
			],
		)];
		drop_disabled_down(&mut groups, |_| {
			panic!("is_enabled must not be probed for Up expectations")
		});
		assert_eq!(groups[0].1.len(), 2);
	}
}
