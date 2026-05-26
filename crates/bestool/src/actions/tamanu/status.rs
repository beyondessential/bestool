use clap::Parser;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table, presets::UTF8_HORIZONTAL_ONLY};
use miette::{IntoDiagnostic, Result, bail};
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

	/// Include compliant legacy expectations (e.g. `tamanu-facility`) in
	/// the output. Without this flag, legacy rows are hidden when they're
	/// in their expected state — they only show up if they fail, so the
	/// 90% of deployments that never had the leftover unit don't see a
	/// permanent OK row for it. Implied when name filters are supplied.
	#[arg(long)]
	pub all: bool,
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
	if !args.all && args.names.is_empty() {
		hide_compliant_legacy(&mut groups);
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

	let (table, any_short) = render_table(&groups, use_colours);
	println!("{table}");

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
	reason: String,
	legacy: bool,
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

/// Drop `legacy` expectations whose outcome is OK from the groups list, so
/// the renderer doesn't print a permanent green row for state that never
/// applied to this deployment. A non-OK legacy expectation (e.g. the
/// long-defunct `tamanu-facility` unit somehow still running) is *kept* so
/// operators still get the prompt to clean it up.
fn hide_compliant_legacy(groups: &mut Vec<(&Expectation, Vec<Instance>)>) {
	groups.retain(|(exp, instances)| !exp.legacy || classify(exp, instances).is_short());
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
				reason: exp.reason.clone(),
				legacy: exp.legacy,
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

/// Per-expectation outcome, used to pick the colour and structure of the
/// "Actual" cell. Drives both the table render and the bail-out at the end
/// of `run` — `is_short` returns true for every non-OK variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Outcome {
	/// `Up` and at least `min_count` instances running.
	Up,
	/// `Up` but some — not all — instances are not running.
	Short,
	/// `Up` but no instances running (either missing entirely or all stopped).
	Down,
	/// `Down` and nothing present (after `drop_disabled_down` ran).
	Absent,
	/// `Down` but one or more instances are present in a meaningful way
	/// (running, or stopped+enabled). The matching units are surfaced in the
	/// Actual cell.
	Forbidden,
}

impl Outcome {
	fn is_short(self) -> bool {
		!matches!(self, Outcome::Up | Outcome::Absent)
	}

	fn colour(self) -> Color {
		match self {
			Outcome::Up | Outcome::Absent => Color::Green,
			Outcome::Short => Color::Yellow,
			Outcome::Down | Outcome::Forbidden => Color::Red,
		}
	}
}

fn classify(exp: &Expectation, instances: &[Instance]) -> Outcome {
	let running = instances.iter().filter(|i| i.running).count();
	match exp.state {
		ExpectedState::Up => {
			let needed = exp.instances.min_count();
			if running >= needed {
				Outcome::Up
			} else if running == 0 {
				Outcome::Down
			} else {
				Outcome::Short
			}
		}
		ExpectedState::Down => {
			if instances.is_empty() {
				Outcome::Absent
			} else {
				Outcome::Forbidden
			}
		}
	}
}

/// Build a table with Service / Expected / Actual / Reason columns from the
/// grouped discovery output. Returns the table and an `any_short` flag the
/// caller uses to set the exit status.
fn render_table(groups: &[(&Expectation, Vec<Instance>)], use_colours: bool) -> (Table, bool) {
	let mut table = Table::new();
	table
		.load_preset(UTF8_HORIZONTAL_ONLY)
		.set_content_arrangement(ContentArrangement::Dynamic)
		.set_header(header_cells(use_colours));

	let mut any_short = false;
	for (exp, instances) in groups {
		let outcome = classify(exp, instances);
		if outcome.is_short() {
			any_short = true;
		}
		table.add_row(vec![
			service_cell(exp),
			expected_cell(exp),
			actual_cell(exp, instances, outcome, use_colours),
			reason_cell(&exp.reason, use_colours),
		]);
	}
	(table, any_short)
}

fn header_cells(use_colours: bool) -> Vec<Cell> {
	["Service", "Expected", "Actual", "Reason"]
		.iter()
		.map(|s| {
			let c = Cell::new(s);
			if use_colours {
				c.add_attribute(Attribute::Bold)
			} else {
				c
			}
		})
		.collect()
}

fn service_cell(exp: &Expectation) -> Cell {
	let suffix = match (exp.state, exp.criticality) {
		(ExpectedState::Up, Criticality::Critical) => " (critical)",
		_ => "",
	};
	Cell::new(format!("{}{suffix}", exp.name))
}

fn expected_cell(exp: &Expectation) -> Cell {
	let text = match exp.state {
		ExpectedState::Up => {
			let n = exp.instances.min_count();
			if n == 1 {
				"up".to_string()
			} else {
				format!("up \u{00d7}{n}")
			}
		}
		ExpectedState::Down => "absent".to_string(),
	};
	Cell::new(text)
}

fn actual_cell(
	exp: &Expectation,
	instances: &[Instance],
	outcome: Outcome,
	use_colours: bool,
) -> Cell {
	let running = instances.iter().filter(|i| i.running).count();
	let needed = exp.instances.min_count();

	let summary = match outcome {
		Outcome::Up => {
			if needed == 1 {
				"running".to_string()
			} else {
				format!("{running}/{needed} running")
			}
		}
		Outcome::Short => format!("{running}/{needed} running"),
		Outcome::Down => {
			if instances.is_empty() {
				"missing".to_string()
			} else {
				format!("0/{needed} running")
			}
		}
		Outcome::Absent => "absent".to_string(),
		Outcome::Forbidden => forbidden_summary(instances),
	};

	let mut lines = vec![summary];
	let want_details = match outcome {
		Outcome::Short | Outcome::Down => instances.len() > 1 || needed > 1,
		Outcome::Forbidden => instances.len() > 1,
		_ => false,
	};
	if want_details {
		for inst in instances {
			lines.push(format!(
				"  {}: {}",
				inst.display(),
				instance_word(inst, exp.state)
			));
		}
	}

	let cell = Cell::new(lines.join("\n"));
	if use_colours {
		cell.fg(outcome.colour())
	} else {
		cell
	}
}

fn forbidden_summary(instances: &[Instance]) -> String {
	if instances.len() == 1 {
		instance_word(&instances[0], ExpectedState::Down).to_string()
	} else {
		let running = instances.iter().filter(|i| i.running).count();
		let stopped = instances.len() - running;
		match (running, stopped) {
			(0, _) => format!("{stopped} stopped, but still enabled"),
			(_, 0) => format!("{running} running"),
			_ => format!("{running} running, {stopped} stopped but still enabled"),
		}
	}
}

/// Short status word for one instance in the Actual cell.
///
/// Surviving stopped instances of a `Down` expectation are known to be
/// enabled (otherwise `drop_disabled_down` would have removed them), so we
/// flag that explicitly — it's the actionable detail that distinguishes a
/// false-positive from "the operator forgot to disable this".
fn instance_word(inst: &Instance, expected: ExpectedState) -> &'static str {
	match (inst.running, expected) {
		(true, _) => "running",
		(false, ExpectedState::Down) => "stopped, but still enabled",
		(false, ExpectedState::Up) => "stopped",
	}
}

fn reason_cell(reason: &str, use_colours: bool) -> Cell {
	let cell = Cell::new(reason);
	if use_colours {
		cell.add_attribute(Attribute::Dim)
	} else {
		cell
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
			legacy: false,
			behind_caddy: false,
		}
	}

	fn up_exp() -> Expectation {
		Expectation {
			name: "tamanu-frontend",
			instances: Instances::Named(&["a", "b"]),
			state: ExpectedState::Up,
			criticality: Criticality::Critical,
			reason: "test".into(),
			legacy: false,
			behind_caddy: false,
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

	fn ok_up_exp() -> Expectation {
		Expectation {
			name: "tamanu-central-tasks",
			instances: Instances::Single,
			state: ExpectedState::Up,
			criticality: Criticality::Background,
			reason: "always required".into(),
			legacy: false,
			behind_caddy: false,
		}
	}

	#[test]
	fn render_table_sample_output() {
		// Snapshot of the realistic mixed-status output the user would see
		// in production: a healthy singleton Up, a critical multi-instance
		// Up, an absent Down (good), and a stopped+enabled Down (the
		// FORBIDDEN false-positive case that motivated this work).
		let tasks = ok_up_exp();
		let frontend = up_exp();
		let absent_down = Expectation {
			name: "tamanu-facility",
			instances: Instances::Single,
			state: ExpectedState::Down,
			criticality: Criticality::Background,
			reason: "legacy singleton unit must not be present".into(),
			legacy: true,
			behind_caddy: false,
		};
		let portal = Expectation {
			name: "tamanu-patientportal",
			instances: Instances::Single,
			state: ExpectedState::Down,
			criticality: Criticality::Background,
			reason: "DB setting features.patientPortal is false".into(),
			legacy: false,
			behind_caddy: false,
		};
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![
			(&tasks, vec![inst("tamanu-central-tasks", None, true)]),
			(
				&frontend,
				vec![
					inst("tamanu-frontend", Some("a"), true),
					inst("tamanu-frontend", Some("b"), true),
				],
			),
			(&absent_down, vec![]),
			(&portal, vec![inst("tamanu-patientportal", None, false)]),
		];
		let (table, any_short) = render_table(&groups, false);
		assert!(any_short, "stopped+enabled Down expectation should bail");
		let rendered = table.to_string();
		assert!(rendered.contains("tamanu-central-tasks"));
		assert!(rendered.contains("tamanu-frontend (critical)"));
		assert!(rendered.contains("up \u{00d7}2"));
		assert!(rendered.contains("tamanu-facility"));
		assert!(rendered.contains("absent"));
		assert!(rendered.contains("stopped, but still enabled"));
		assert!(rendered.contains("always required"));
		assert!(
			rendered.contains("features.patientPortal"),
			"reason column should show portal's DB reason"
		);
	}

	#[test]
	fn render_table_short_up_lists_per_instance() {
		// Multi-instance Up with one instance stopped should expand the
		// Actual cell to show each instance's state — the summary alone
		// ("1/2 running") doesn't tell the operator which one to start.
		let exp = up_exp();
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&exp,
			vec![
				inst("tamanu-frontend", Some("a"), true),
				inst("tamanu-frontend", Some("b"), false),
			],
		)];
		let (table, any_short) = render_table(&groups, false);
		assert!(any_short);
		let rendered = table.to_string();
		assert!(rendered.contains("1/2 running"));
		assert!(rendered.contains("tamanu-frontend@a: running"));
		assert!(rendered.contains("tamanu-frontend@b: stopped"));
	}

	#[test]
	fn hide_compliant_legacy_drops_absent_legacy_row() {
		// Legacy Down expectation, nothing present → compliant → hidden.
		let legacy = Expectation {
			name: "tamanu-facility",
			instances: Instances::Single,
			state: ExpectedState::Down,
			criticality: Criticality::Background,
			reason: "legacy singleton unit must not be present".into(),
			legacy: true,
			behind_caddy: false,
		};
		let tasks = ok_up_exp();
		let mut groups: Vec<(&Expectation, Vec<Instance>)> = vec![
			(&legacy, vec![]),
			(&tasks, vec![inst("tamanu-central-tasks", None, true)]),
		];
		hide_compliant_legacy(&mut groups);
		assert_eq!(groups.len(), 1);
		assert_eq!(groups[0].0.name, "tamanu-central-tasks");
	}

	#[test]
	fn hide_compliant_legacy_keeps_failing_legacy_row() {
		// Non-compliant legacy (running when it should be absent) must
		// stay visible — that's exactly the case the check exists to catch.
		let legacy = Expectation {
			name: "tamanu-facility",
			instances: Instances::Single,
			state: ExpectedState::Down,
			criticality: Criticality::Background,
			reason: "legacy singleton unit must not be present".into(),
			legacy: true,
			behind_caddy: false,
		};
		let mut groups: Vec<(&Expectation, Vec<Instance>)> =
			vec![(&legacy, vec![inst("tamanu-facility", None, true)])];
		hide_compliant_legacy(&mut groups);
		assert_eq!(groups.len(), 1);
		assert_eq!(groups[0].0.name, "tamanu-facility");
	}

	#[test]
	fn hide_compliant_legacy_keeps_non_legacy_compliant_rows() {
		// Non-legacy expectations are never filtered, even when compliant.
		let portal = down_exp();
		let mut groups: Vec<(&Expectation, Vec<Instance>)> = vec![(&portal, vec![])];
		hide_compliant_legacy(&mut groups);
		assert_eq!(groups.len(), 1, "non-legacy Down/absent must remain shown");
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
