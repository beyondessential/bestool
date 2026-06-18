use std::collections::{HashMap, HashSet};

use clap::Parser;
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table, presets::UTF8_HORIZONTAL_ONLY};
use miette::{IntoDiagnostic, Result, bail};
use serde::Serialize;
use tracing::warn;

use bestool_tamanu::{
	services::{self, ExpectedState, Expectation, Supervisor},
	systemd,
	versions::{self, ExpectedVersions, VersionStatus},
};

use crate::actions::{
	Context,
	tamanu::{
		TamanuArgs, find_tamanu,
		lifecycle::{self, Instance, WaitForDb},
	},
};

/// Report on tamanu services: what's expected vs what's actually running.
///
/// A lighter cousin of `tamanu doctor`: discovery only, no HTTP probes.
/// Useful as a quick "is anything down right now?" check, or before/after a
/// `tamanu start` / `restart` to see the impact.
///
/// Re-execs under sudo when not already root: reading each service's running
/// version means inspecting its (root-owned) podman container, which an
/// unprivileged process can't see.
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

	let (supervisor, expectations) =
		lifecycle::config_and_expectations(tamanu, WaitForDb::No).await?;
	let names: Vec<&str> = args.names.iter().map(String::as_str).collect();
	let matched = services::match_names(&expectations, &names, false)?;
	let discovered = lifecycle::discover(supervisor).await?;
	let mut groups = lifecycle::group_by_expectation(supervisor, &matched, &discovered);
	if matches!(supervisor, Supervisor::Systemd) {
		let candidates: HashSet<String> = groups
			.iter()
			.filter(|(exp, _)| matches!(exp.state, ExpectedState::Down))
			.flat_map(|(_, instances)| instances.iter().map(Instance::unit))
			.collect();
		let enabled = systemd::collect_enabled(candidates).await;
		drop_disabled_down(&mut groups, |unit| enabled.contains(unit));
	}
	if !args.all && args.names.is_empty() {
		hide_compliant_legacy(&mut groups);
	}

	// Reading each instance's *running* version means inspecting its podman
	// container, and on these deployments the containers are root-owned — a
	// `podman ps` as a normal user sees nothing, so every actual version comes
	// back "unknown" and only the expected (env-file) version is left on show.
	// Discovery above works unprivileged via systemd's D-Bus; elevate here, the
	// same way the mutating lifecycle commands do, so the version probe below
	// can actually see what's running.
	lifecycle::ensure_root_or_reexec(supervisor)?;

	// Probe version expected vs running. Both are best-effort:
	// missing data degrades cleanly to "unknown" rather than failing the
	// status check, since the same data sources already power other (more
	// authoritative) drift signals upstream.
	let (install_version, _root) = find_tamanu(tamanu).await?;
	let expected_versions = versions::expected_for_supervisor(supervisor, &install_version);
	// `Err` here means we couldn't read podman at all (vs. an empty map, which
	// means nothing is running). Keep the reason so the render can say the
	// Version column is showing expected-only rather than silently presenting
	// the configured version as if it were confirmed running.
	let (running_versions, running_error) = match supervisor {
		Supervisor::Systemd => match versions::running_versions_linux().await {
			Ok(map) => (map, None),
			Err(reason) => (HashMap::new(), Some(reason)),
		},
		Supervisor::Pm2 => (HashMap::new(), None),
	};
	if let Some(reason) = &running_error {
		warn!(%reason, "could not read running container versions; Version column shows expected only");
	}
	let probe = VersionProbe {
		supervisor,
		expected: expected_versions,
		running: running_versions,
		install: install_version.to_string(),
	};

	if args.json {
		let report = build_report(&groups, &probe);
		println!(
			"{}",
			serde_json::to_string_pretty(&report).into_diagnostic()?
		);
		if report.any_short {
			bail!("some expectations are not met");
		}
		return Ok(());
	}

	let (table, any_short) = render_table(&groups, &probe, use_colours);
	println!("{table}");

	if let Some(reason) = &running_error {
		println!(
			"\nNote: couldn't read running container versions ({reason}); the Version \
			 column shows the configured (expected) versions only, not what's live."
		);
	}

	if any_short {
		bail!("some expectations are not met");
	}
	Ok(())
}

/// Bundle of resolved version data — passed to renderers so they can ask
/// per-instance "what version is this on, and does it match?".
struct VersionProbe {
	supervisor: Supervisor,
	expected: ExpectedVersions,
	/// Linux: unit name → image tag. Empty on pm2.
	running: HashMap<String, String>,
	/// Install-root version, used as the actual version for every pm2
	/// instance (pm2 has no per-process version concept).
	install: String,
}

impl VersionProbe {
	fn expected_for(&self, expectation_name: &str) -> Option<&str> {
		self.expected.for_service(expectation_name)
	}

	fn actual_for(&self, instance: &Instance) -> Option<String> {
		match self.supervisor {
			Supervisor::Systemd => {
				if instance.running {
					self.running.get(&instance.unit()).cloned()
				} else {
					None
				}
			}
			Supervisor::Pm2 if instance.running => Some(self.install.clone()),
			Supervisor::Pm2 => None,
		}
	}

	fn classify(&self, expectation: &Expectation, instance: &Instance) -> VersionStatus {
		versions::classify(
			self.actual_for(instance).as_deref(),
			self.expected_for(expectation.name),
		)
	}
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
	running: usize,
	min_count: usize,
	status: &'static str,
	reason: String,
	legacy: bool,
	behind_caddy: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	expected_version: Option<String>,
	instances: Vec<InstanceReport>,
}

#[derive(Serialize)]
struct InstanceReport {
	name: String,
	instance: Option<String>,
	pm_id: Option<i64>,
	running: bool,
	#[serde(skip_serializing_if = "Option::is_none")]
	version_actual: Option<String>,
	version_status: &'static str,
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

fn build_report(groups: &[(&Expectation, Vec<Instance>)], probe: &VersionProbe) -> Report {
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
				// Unknown is *not* short: we deliberately don't know what the
				// expectation should be (typically because the DB-derived
				// signal was unreachable), so there's nothing to act on or
				// alarm about. The row still appears in the table so
				// operators see the gap.
				ExpectedState::Unknown => "unknown",
			};
			let expected_version = probe.expected_for(exp.name).map(str::to_string);
			ExpectationReport {
				name: exp.name,
				expected_state: match exp.state {
					ExpectedState::Up => "up",
					ExpectedState::Down => "down",
					ExpectedState::Unknown => "unknown",
				},
				running,
				min_count: exp.instances.min_count(),
				status,
				reason: exp.reason.clone(),
				legacy: exp.legacy,
				behind_caddy: exp.behind_caddy,
				expected_version,
				instances: instances
					.iter()
					.map(|i| {
						let actual = probe.actual_for(i);
						let version_status = probe.classify(exp, i);
						if version_status.is_mismatch() {
							any_short = true;
						}
						InstanceReport {
							name: i.name.clone(),
							instance: i.instance.clone(),
							pm_id: i.pm_id,
							running: i.running,
							version_actual: actual,
							version_status: match version_status {
								VersionStatus::Match => "match",
								VersionStatus::Mismatch => "mismatch",
								VersionStatus::Unknown => "unknown",
							},
						}
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
	/// Expectation state was `Unknown` — the driving signal (e.g. a DB
	/// flag) couldn't be read. We don't claim the actual state is anything
	/// in particular; the row is rendered but isn't a failure.
	Unknown,
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
			// Coloured Yellow rather than Red so the operator sees the row
			// is worth attention but not a failure. The driving signal (DB
			// flag) couldn't be read, so anything could be true.
			Outcome::Unknown => Color::Yellow,
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
		// We deliberately don't know what should be running — surface the
		// row so operators see the gap, but classify it as Unknown rather
		// than guessing Up/Down.
		ExpectedState::Unknown => Outcome::Unknown,
	}
}

/// Build a table with Service / Expected / Actual / Version / Reason columns
/// from the grouped discovery output. Returns the table and an `any_short`
/// flag the caller uses to set the exit status (version drift contributes to
/// it just like a stopped service).
fn render_table(
	groups: &[(&Expectation, Vec<Instance>)],
	probe: &VersionProbe,
	use_colours: bool,
) -> (Table, bool) {
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
		let (version_cell_, any_drift) = version_cell(exp, instances, probe, use_colours);
		if any_drift {
			any_short = true;
		}
		table.add_row(vec![
			service_cell(exp),
			expected_cell(exp),
			actual_cell(exp, instances, outcome, use_colours),
			version_cell_,
			reason_cell(&exp.reason, use_colours),
		]);
	}
	(table, any_short)
}

fn header_cells(use_colours: bool) -> Vec<Cell> {
	["Service", "Expected", "Actual", "Version", "Reason"]
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
	Cell::new(exp.name)
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
		ExpectedState::Unknown => "unknown".to_string(),
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
		Outcome::Unknown => {
			// Surface whatever's actually there, without claiming it
			// matches an expectation. Bare "unknown" would hide useful
			// detail; the running/stopped split is still real.
			if instances.is_empty() {
				"unknown (nothing present)".to_string()
			} else if running == instances.len() {
				format!("unknown ({running} running)")
			} else if running == 0 {
				format!("unknown ({} stopped)", instances.len())
			} else {
				let stopped = instances.len() - running;
				format!("unknown ({running} running, {stopped} stopped)")
			}
		}
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
		// We don't know whether stopped is right or wrong, so just say so.
		(false, ExpectedState::Unknown) => "stopped",
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

/// Build the Version cell. Summary on the first line; per-instance detail
/// follows when there's more than one instance or when at least one is
/// mismatched (so the operator can tell *which* one drifted). Returns
/// `(cell, any_drift)` where `any_drift` is whether any running instance
/// is on a different tag than expected — bubbled up to set the exit code.
fn version_cell(
	exp: &Expectation,
	instances: &[Instance],
	probe: &VersionProbe,
	use_colours: bool,
) -> (Cell, bool) {
	let expected = probe.expected_for(exp.name);
	let per_instance: Vec<(&Instance, Option<String>, VersionStatus)> = instances
		.iter()
		.map(|i| (i, probe.actual_for(i), probe.classify(exp, i)))
		.collect();

	let any_drift = per_instance
		.iter()
		.any(|(_, _, status)| status.is_mismatch());

	if instances.is_empty() {
		// `Down` expectations with no instances — version column has nothing
		// meaningful to say.
		return (Cell::new("—"), false);
	}

	let summary = match expected {
		Some(v) => v.to_string(),
		None => "?".to_string(),
	};
	let show_details = instances.len() > 1 || any_drift;
	let mut lines = vec![summary];
	if show_details {
		for (inst, actual, status) in &per_instance {
			let actual_str = actual.as_deref().unwrap_or("?");
			let suffix = match status {
				VersionStatus::Match => "ok",
				VersionStatus::Mismatch => "drift",
				VersionStatus::Unknown => "unknown",
			};
			lines.push(format!("  {}: {} ({})", inst.display(), actual_str, suffix));
		}
	}

	let cell = Cell::new(lines.join("\n"));
	let cell = if use_colours {
		let colour = if any_drift {
			Color::Red
		} else if per_instance
			.iter()
			.all(|(_, _, status)| matches!(status, VersionStatus::Match))
		{
			Color::Green
		} else {
			Color::Yellow
		};
		cell.fg(colour)
	} else {
		cell
	};
	(cell, any_drift)
}

#[cfg(test)]
mod tests {
	use super::*;
	use bestool_tamanu::services::Instances;

	fn down_exp() -> Expectation {
		Expectation {
			name: "tamanu-patientportal",
			instances: Instances::Named(&["a", "b"]),
			state: ExpectedState::Down,
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

	fn empty_probe() -> VersionProbe {
		VersionProbe {
			supervisor: Supervisor::Systemd,
			expected: ExpectedVersions::default(),
			running: HashMap::new(),
			install: String::new(),
		}
	}

	fn probe_with(expected: &str, running: &[(&str, &str)]) -> VersionProbe {
		VersionProbe {
			supervisor: Supervisor::Systemd,
			expected: ExpectedVersions {
				tamanu: Some(expected.into()),
				frontend: None,
			},
			running: running
				.iter()
				.map(|(k, v)| (k.to_string(), v.to_string()))
				.collect(),
			install: expected.into(),
		}
	}

	#[test]
	fn drop_disabled_down_removes_stopped_and_disabled() {
		let exp = down_exp();
		let mut groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&exp,
			vec![
				inst("tamanu-patientportal", Some("a"), false),
				inst("tamanu-patientportal", Some("b"), false),
			],
		)];
		drop_disabled_down(&mut groups, |_| false);
		assert!(
			groups[0].1.is_empty(),
			"stopped+disabled Down units should all be dropped",
		);
		let report = build_report(&groups, &empty_probe());
		assert!(!report.any_short, "should be ABSENT/OK now");
		assert_eq!(report.expectations[0].status, "absent");
	}

	#[test]
	fn drop_disabled_down_keeps_stopped_but_enabled() {
		let exp = down_exp();
		let mut groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&exp,
			vec![
				inst("tamanu-patientportal", Some("a"), false),
				inst("tamanu-patientportal", Some("b"), false),
			],
		)];
		drop_disabled_down(&mut groups, |_| true);
		assert_eq!(groups[0].1.len(), 2);
		let report = build_report(&groups, &empty_probe());
		assert!(report.any_short, "stopped+enabled is still FORBIDDEN");
		assert_eq!(report.expectations[0].status, "forbidden");
	}

	fn ok_up_exp() -> Expectation {
		Expectation {
			name: "tamanu-central-tasks",
			instances: Instances::Single,
			state: ExpectedState::Up,
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
			reason: "legacy singleton unit must not be present".into(),
			legacy: true,
			behind_caddy: false,
		};
		let portal = Expectation {
			name: "tamanu-patientportal",
			instances: Instances::Named(&["a", "b"]),
			state: ExpectedState::Down,
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
			(
				&portal,
				vec![
					inst("tamanu-patientportal", Some("a"), false),
					inst("tamanu-patientportal", Some("b"), false),
				],
			),
		];
		let (table, any_short) = render_table(&groups, &empty_probe(), false);
		assert!(any_short, "stopped+enabled Down expectation should bail");
		let rendered = table.to_string();
		assert!(rendered.contains("tamanu-central-tasks"));
		assert!(rendered.contains("tamanu-frontend"));
		assert!(rendered.contains("up \u{00d7}2"));
		assert!(rendered.contains("tamanu-facility"));
		assert!(rendered.contains("absent"));
		assert!(rendered.contains("stopped, but still enabled"));
		assert!(rendered.contains("always required"));
		assert!(
			rendered.contains("features.patientPortal"),
			"reason column should show portal's DB reason"
		);
		assert!(rendered.contains("Version"), "Version column header present");
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
		let (table, any_short) = render_table(&groups, &empty_probe(), false);
		assert!(any_short);
		let rendered = table.to_string();
		assert!(rendered.contains("1/2 running"));
		assert!(rendered.contains("tamanu-frontend@a: running"));
		assert!(rendered.contains("tamanu-frontend@b: stopped"));
	}

	#[test]
	fn version_drift_marks_any_short() {
		// One frontend instance on the expected tag, another on a stale
		// tag — the drift alone must bail, even though both instances are
		// "running".
		let exp = up_exp();
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&exp,
			vec![
				inst("tamanu-frontend", Some("a"), true),
				inst("tamanu-frontend", Some("b"), true),
			],
		)];
		let probe = probe_with(
			"v2.10.0",
			&[
				("tamanu-frontend@a.service", "v2.10.0"),
				("tamanu-frontend@b.service", "v2.9.5"),
			],
		);
		let report = build_report(&groups, &probe);
		assert!(report.any_short, "drift should fail the check");
		let inst_reports = &report.expectations[0].instances;
		assert_eq!(inst_reports[0].version_status, "match");
		assert_eq!(inst_reports[1].version_status, "mismatch");
		assert_eq!(inst_reports[1].version_actual.as_deref(), Some("v2.9.5"));

		let (table, any_short) = render_table(&groups, &probe, false);
		assert!(any_short);
		let rendered = table.to_string();
		assert!(rendered.contains("v2.10.0"));
		assert!(rendered.contains("v2.9.5"));
		assert!(rendered.contains("drift"));
	}

	#[test]
	fn version_match_doesnt_fail() {
		let exp = up_exp();
		let groups: Vec<(&Expectation, Vec<Instance>)> = vec![(
			&exp,
			vec![
				inst("tamanu-frontend", Some("a"), true),
				inst("tamanu-frontend", Some("b"), true),
			],
		)];
		let probe = probe_with(
			"v2.10.0",
			&[
				("tamanu-frontend@a.service", "v2.10.0"),
				("tamanu-frontend@b.service", "v2.10.0"),
			],
		);
		let report = build_report(&groups, &probe);
		assert!(!report.any_short);
		assert!(
			report.expectations[0]
				.instances
				.iter()
				.all(|i| i.version_status == "match")
		);
	}

	#[test]
	fn version_unknown_doesnt_fail() {
		// `running_versions_linux` returned nothing (podman down). Don't
		// flag mismatch — the actual is just not known.
		let exp = ok_up_exp(); // singleton, so running=1 satisfies min_count
		let groups: Vec<(&Expectation, Vec<Instance>)> =
			vec![(&exp, vec![inst("tamanu-central-tasks", None, true)])];
		let probe = probe_with("v2.10.0", &[]);
		let report = build_report(&groups, &probe);
		assert!(!report.any_short, "unknown isn't a failure");
		assert_eq!(report.expectations[0].instances[0].version_status, "unknown");
	}

	#[test]
	fn hide_compliant_legacy_drops_absent_legacy_row() {
		// Legacy Down expectation, nothing present → compliant → hidden.
		let legacy = Expectation {
			name: "tamanu-facility",
			instances: Instances::Single,
			state: ExpectedState::Down,
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
