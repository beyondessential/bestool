use std::io::{self, Write};

use owo_colors::OwoColorize;

use bestool_alertd::doctor::check::{Check, CheckStatus, OverallResult};

use super::{SweepSource, order};

/// Render the full human-readable output: grouped check list, blank line,
/// result line, and dimmed source note. `results` is the complete, sorted set;
/// the displayed list hides passing and skipped checks unless `show_all` is set
/// (so a clean sweep shows nothing), but the result line always counts the full
/// set regardless of what is displayed.
pub fn render_plain<W: Write>(
	out: &mut W,
	results: &[(Check, bool)],
	show_all: bool,
	overall: OverallResult,
	source: &SweepSource,
	use_colours: bool,
) -> io::Result<()> {
	let displayed: Vec<&(Check, bool)> = results
		.iter()
		.filter(|(c, _)| order::keep_in_replay(&c.status, show_all))
		.collect();
	let width = displayed
		.iter()
		.map(|(c, _)| c.name.len())
		.max()
		.unwrap_or(0);
	for (check, _) in &displayed {
		write_check_line(out, check, width, use_colours)?;
	}
	if !displayed.is_empty() {
		writeln!(out)?;
	}
	write_result_line(out, results, overall, use_colours)?;
	write_source_note(out, source, use_colours)?;
	Ok(())
}

pub fn write_check_line<W: Write>(
	out: &mut W,
	check: &Check,
	name_width: usize,
	use_colours: bool,
) -> io::Result<()> {
	let tag = match &check.status {
		CheckStatus::Pass => colour_pass(use_colours, "PASS"),
		CheckStatus::Skip(_) => colour_skip(use_colours, "SKIP"),
		CheckStatus::Warning(_) => colour_warn(use_colours, "WARN"),
		CheckStatus::Fail(_) => colour_fail(use_colours, "FAIL"),
		CheckStatus::Broken(_) => colour_broken(use_colours, "BRKN"),
	};
	writeln!(
		out,
		"  {tag}    {name:<width$}   {summary}",
		name = check.name,
		width = name_width,
		summary = check.summary,
	)?;
	if let CheckStatus::Skip(r)
	| CheckStatus::Warning(r)
	| CheckStatus::Fail(r)
	| CheckStatus::Broken(r) = &check.status
	{
		let dim = if use_colours {
			format!("{}", r.dimmed())
		} else {
			r.clone()
		};
		writeln!(
			out,
			"          {empty:<width$}     {dim}",
			empty = "",
			width = name_width,
		)?;
	}
	Ok(())
}

pub fn write_result_line<W: Write>(
	out: &mut W,
	results: &[(Check, bool)],
	overall: OverallResult,
	use_colours: bool,
) -> io::Result<()> {
	let (mut passes, mut warnings, mut fails, mut skips, mut brokens) =
		(0usize, 0usize, 0usize, 0usize, 0usize);
	for (check, _) in results {
		match &check.status {
			CheckStatus::Pass => passes += 1,
			CheckStatus::Skip(_) => skips += 1,
			CheckStatus::Warning(_) => warnings += 1,
			CheckStatus::Fail(_) => fails += 1,
			CheckStatus::Broken(_) => brokens += 1,
		}
	}
	let label = overall.label();
	let label_coloured = match overall {
		OverallResult::Healthy => colour_pass(use_colours, label),
		OverallResult::Degraded => colour_warn(use_colours, label),
		OverallResult::Failing => colour_fail(use_colours, label),
	};
	writeln!(
		out,
		"Result: {label_coloured} ({passes} passed, {fails} failed, {warnings} warning{plural}, {brokens} broken, {skips} skipped)",
		plural = if warnings == 1 { "" } else { "s" },
	)
}

pub fn write_source_note<W: Write>(
	out: &mut W,
	source: &SweepSource,
	use_colours: bool,
) -> io::Result<()> {
	let line = match source {
		SweepSource::Local => return Ok(()),
		SweepSource::DaemonStreamed => "Source: alertd daemon (just now, on demand)".to_string(),
		SweepSource::DaemonCached { computed_at } => {
			let age = humanise_age_since(*computed_at);
			format!("Source: alertd daemon (computed {age} ago, at {computed_at})")
		}
	};
	if use_colours {
		writeln!(out, "{}", line.dimmed())
	} else {
		writeln!(out, "{line}")
	}
}

pub fn humanise_age_since(then: jiff::Timestamp) -> String {
	let now = jiff::Timestamp::now();
	let secs = now.as_second().saturating_sub(then.as_second()).max(0) as u64;
	if secs < 60 {
		format!("{secs}s")
	} else if secs < 3600 {
		format!("{}m {}s", secs / 60, secs % 60)
	} else {
		format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
	}
}

fn colour_pass(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.green().bold())
	} else {
		s.to_string()
	}
}

fn colour_skip(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.dimmed().bold())
	} else {
		s.to_string()
	}
}

fn colour_warn(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.yellow().bold())
	} else {
		s.to_string()
	}
}

fn colour_fail(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.red().bold())
	} else {
		s.to_string()
	}
}

fn colour_broken(use_colours: bool, s: &str) -> String {
	if use_colours {
		format!("{}", s.magenta().bold())
	} else {
		s.to_string()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::actions::tamanu::doctor::order::filter_and_sort;

	fn pass(name: &'static str) -> (Check, bool) {
		(Check::pass(name, "ok"), true)
	}
	fn warn(name: &'static str) -> (Check, bool) {
		(Check::warning(name, "deg", "reason"), true)
	}
	fn fail(name: &'static str) -> (Check, bool) {
		(Check::fail(name, "bad", "reason"), true)
	}
	fn skip(name: &'static str) -> (Check, bool) {
		(Check::skip(name, "not run", "reason"), true)
	}
	fn broken(name: &'static str) -> (Check, bool) {
		(Check::broken(name, "broke", "reason"), true)
	}

	#[test]
	fn render_plain_lists_results_in_severity_order() {
		let raw = vec![fail("z-fail"), pass("a-pass"), warn("m-warn")];
		let sorted = filter_and_sort(&raw, true);
		let overall = OverallResult::from_checks(&sorted.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let mut buf = Vec::new();
		render_plain(&mut buf, &sorted, true, overall, &SweepSource::Local, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		let pass_pos = out.find("a-pass").unwrap();
		let warn_pos = out.find("m-warn").unwrap();
		let fail_pos = out.find("z-fail").unwrap();
		assert!(pass_pos < warn_pos);
		assert!(warn_pos < fail_pos);
		assert!(out.contains("FAILING"));
		assert!(out.contains("1 failed"));
	}

	#[test]
	fn render_plain_default_shows_just_result_line_when_clean() {
		let raw = vec![pass("a"), pass("b"), skip("c")];
		let sorted = filter_and_sort(&raw, true);
		let overall = OverallResult::from_checks(&raw.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let mut buf = Vec::new();
		render_plain(&mut buf, &sorted, false, overall, &SweepSource::Local, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert!(!out.contains("PASS"));
		assert!(!out.contains("SKIP"));
		assert!(out.contains("HEALTHY"));
		// The result line still reports the full counts even though nothing is listed.
		assert!(out.contains("2 passed"));
		assert!(out.contains("1 skipped"));
	}

	#[test]
	fn render_plain_default_keeps_warn_broken_fail() {
		let raw = vec![pass("a"), warn("b"), broken("c"), fail("d"), skip("e")];
		let sorted = filter_and_sort(&raw, true);
		let overall = OverallResult::from_checks(&raw.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let mut buf = Vec::new();
		render_plain(&mut buf, &sorted, false, overall, &SweepSource::Local, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert!(out.contains("WARN"));
		assert!(out.contains("BRKN"));
		assert!(out.contains("FAIL"));
		assert!(!out.contains("PASS"));
		assert!(!out.contains("SKIP"));
		// Order: warn (b), broken (c), fail (d)
		let warn_pos = out.find("WARN").unwrap();
		let broken_pos = out.find("BRKN").unwrap();
		let fail_pos = out.find("FAIL").unwrap();
		assert!(warn_pos < broken_pos);
		assert!(broken_pos < fail_pos);
	}

	#[test]
	fn render_plain_no_server_id_header() {
		let raw = vec![pass("a")];
		let sorted = filter_and_sort(&raw, true);
		let overall = OverallResult::from_checks(&raw.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let mut buf = Vec::new();
		render_plain(&mut buf, &sorted, true, overall, &SweepSource::Local, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert!(!out.contains("server-id"));
		assert!(!out.contains("Server:"));
	}

	#[test]
	fn render_plain_includes_source_note_for_daemon_streamed() {
		let raw = vec![pass("a")];
		let sorted = filter_and_sort(&raw, true);
		let overall = OverallResult::from_checks(&raw.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let mut buf = Vec::new();
		render_plain(&mut buf, &sorted, false, overall, &SweepSource::DaemonStreamed, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert!(out.contains("alertd daemon"));
	}

	#[test]
	fn result_line_always_lists_every_count() {
		let results = vec![broken("a"), skip("b"), pass("c")];
		let overall = OverallResult::from_checks(&results.iter().map(|(c, _)| c.clone()).collect::<Vec<_>>());
		let mut buf = Vec::new();
		write_result_line(&mut buf, &results, overall, false).unwrap();
		let out = String::from_utf8(buf).unwrap();
		assert!(out.contains("1 passed"));
		assert!(out.contains("0 failed"));
		assert!(out.contains("0 warnings"));
		assert!(out.contains("1 broken"));
		assert!(out.contains("1 skipped"));
		assert!(out.contains("DEGRADED"));
	}
}
