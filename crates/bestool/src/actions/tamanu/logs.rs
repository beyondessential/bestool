use std::{
	fmt::Write as _,
	fs::File,
	io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
	path::Path,
	process::{Command, Stdio},
	sync::mpsc::{Sender, channel},
	thread,
	time::Duration,
};

use clap::Parser;
use miette::{IntoDiagnostic, Result, bail, miette};
use owo_colors::OwoColorize;
use regex::Regex;
use serde_json::Value;
use tracing::debug;

use crate::actions::{
	Context,
	tamanu::{
		ApiServerKind, TamanuArgs, config::load_config, find_tamanu,
		pm2::{self, LogSource},
		services::{self, ExpectedState, Expectation, Instances, Supervisor},
	},
};

/// Tail logs for a Tamanu service, in a supervisor-agnostic way.
///
/// On Linux this drives `journalctl -u`; on Windows it reads pm2's log files
/// directly. The service name is matched as a substring against the expected
/// service list, so `tamanu logs api` picks up
/// `tamanu-{central,facility}-api@*` on systemd and `tamanu-api` on pm2.
///
/// The special name `caddy` tails the caddy service (Linux only). Caddy
/// emits JSON-per-line logs; bestool detects these and applies opportunistic
/// syntax highlighting on a TTY.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct LogsArgs {
	/// Service name. Matched as a substring against the expected service list.
	pub name: String,

	/// Number of trailing lines to print before tailing.
	#[arg(short = 'n', long = "lines", default_value = "10")]
	pub lines: usize,

	/// Follow: keep printing new lines as they arrive. Equivalent to `tail -f`.
	#[arg(short = 'f', long = "follow")]
	pub follow: bool,

	/// Only print lines matching this regex. On Linux this is passed to
	/// `journalctl -g`; on Windows it's applied client-side after reading from
	/// the pm2 log files.
	#[arg(short = 'g', long = "grep", value_name = "REGEX")]
	pub grep: Option<Regex>,
}

pub async fn run(ctx: Context<TamanuArgs, LogsArgs>) -> Result<()> {
	if ctx.args_sub.name == "caddy" {
		return run_caddy_logs(
			ctx.args_sub.lines,
			ctx.args_sub.follow,
			ctx.args_sub.grep.as_ref(),
			ctx.args_top.use_colours,
		);
	}

	let (_, root) = find_tamanu(&ctx.args_top)?;
	let config = load_config(&root, None)?;
	let kind = if config.is_facility() {
		ApiServerKind::Facility
	} else {
		ApiServerKind::Central
	};

	let supervisor = if cfg!(target_os = "linux") {
		Supervisor::Systemd
	} else if cfg!(target_os = "windows") {
		Supervisor::Pm2
	} else {
		bail!("tamanu logs is only supported on Linux (systemd) and Windows (pm2)");
	};

	let expectations = services::expected(supervisor, kind, &config);
	let matches: Vec<&Expectation> = match_name(&expectations, &ctx.args_sub.name).collect();

	if matches.is_empty() {
		let candidates: Vec<&str> = up_names(&expectations).collect();
		bail!(
			"no service matches {:?}; expected services are: {}",
			ctx.args_sub.name,
			candidates.join(", ")
		);
	}

	debug!(
		?matches,
		"matched expectations for `tamanu logs {}`",
		ctx.args_sub.name
	);

	match supervisor {
		Supervisor::Systemd => run_journalctl(
			&matches,
			ctx.args_sub.lines,
			ctx.args_sub.follow,
			ctx.args_sub.grep.as_ref(),
		),
		Supervisor::Pm2 => run_pm2_logs(
			&matches,
			ctx.args_sub.lines,
			ctx.args_sub.follow,
			ctx.args_sub.grep,
		),
	}
}

fn up_names(expectations: &[Expectation]) -> impl Iterator<Item = &str> {
	expectations
		.iter()
		.filter(|e| e.state == ExpectedState::Up)
		.map(|e| e.name)
}

fn match_name<'a>(
	expectations: &'a [Expectation],
	needle: &'a str,
) -> impl Iterator<Item = &'a Expectation> {
	expectations
		.iter()
		.filter(|e| e.state == ExpectedState::Up)
		.filter(move |e| e.name.contains(needle))
}

fn run_caddy_logs(
	lines: usize,
	follow: bool,
	grep: Option<&Regex>,
	use_colours: bool,
) -> Result<()> {
	if !cfg!(target_os = "linux") {
		bail!("caddy logs are only available on Linux deployments");
	}

	// --output=cat strips journalctl's prefix (timestamp, host, unit) so we
	// see the raw log lines caddy emits. Caddy puts its own ts into each line.
	let mut cmd = Command::new("journalctl");
	cmd.arg("-u").arg("caddy.service");
	cmd.arg("-n").arg(lines.to_string());
	if follow {
		cmd.arg("-f");
	}
	if let Some(re) = grep {
		cmd.arg("-g").arg(re.as_str());
	}
	cmd.arg("--output=cat");
	cmd.stdout(Stdio::piped());

	let mut child = cmd.spawn().into_diagnostic()?;
	let stdout = child.stdout.take().ok_or_else(|| miette!("no stdout pipe"))?;
	let reader = BufReader::new(stdout);

	let out_handle = std::io::stdout();
	let mut out = out_handle.lock();
	for line in reader.lines() {
		let line = match line {
			Ok(l) => l,
			Err(_) => break,
		};
		let formatted = format_log_line(&line, use_colours);
		if writeln!(out, "{formatted}").is_err() {
			break;
		}
	}
	let status = child.wait().into_diagnostic()?;
	if !status.success() {
		bail!("journalctl exited with {status}");
	}
	Ok(())
}

/// If `line` looks like a JSON object, format it with colored tokens; else
/// return it as-is. `color = false` always returns the line unchanged.
fn format_log_line(line: &str, color: bool) -> String {
	if !color {
		return line.to_string();
	}
	let trimmed = line.trim_start();
	if !trimmed.starts_with('{') {
		return line.to_string();
	}
	let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
		return line.to_string();
	};
	let mut out = String::new();
	write_colored_json(&value, &mut out);
	out
}

fn write_colored_json(v: &Value, out: &mut String) {
	match v {
		Value::Null => {
			let _ = write!(out, "{}", "null".dimmed());
		}
		Value::Bool(b) => {
			let _ = write!(out, "{}", b.bright_magenta());
		}
		Value::Number(n) => {
			let _ = write!(out, "{}", n.to_string().yellow());
		}
		Value::String(s) => {
			let quoted = serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""));
			let _ = write!(out, "{}", quoted.green());
		}
		Value::Array(a) => {
			let _ = write!(out, "{}", "[".dimmed());
			for (i, x) in a.iter().enumerate() {
				if i > 0 {
					let _ = write!(out, "{}", ",".dimmed());
				}
				write_colored_json(x, out);
			}
			let _ = write!(out, "{}", "]".dimmed());
		}
		Value::Object(o) => {
			let _ = write!(out, "{}", "{".dimmed());
			for (i, (k, vv)) in o.iter().enumerate() {
				if i > 0 {
					let _ = write!(out, "{}", ",".dimmed());
				}
				let key = format!("\"{k}\"");
				let _ = write!(out, "{}", key.cyan());
				let _ = write!(out, "{}", ":".dimmed());
				write_colored_json(vv, out);
			}
			let _ = write!(out, "{}", "}".dimmed());
		}
	}
}

fn run_journalctl(
	matches: &[&Expectation],
	lines: usize,
	follow: bool,
	grep: Option<&Regex>,
) -> Result<()> {
	let mut cmd = Command::new("journalctl");
	for m in matches {
		cmd.arg("-u").arg(journalctl_pattern(m));
	}
	cmd.arg("-n").arg(lines.to_string());
	if follow {
		cmd.arg("-f");
	}
	if let Some(re) = grep {
		cmd.arg("-g").arg(re.as_str());
	}
	let status = cmd.status().into_diagnostic()?;
	if !status.success() {
		bail!("journalctl exited with {status}");
	}
	Ok(())
}

fn journalctl_pattern(expectation: &Expectation) -> String {
	match expectation.instances {
		Instances::Single => format!("{}.service", expectation.name),
		Instances::NumericAtLeast(_) | Instances::Named(_) => {
			format!("{}@*.service", expectation.name)
		}
	}
}

fn run_pm2_logs(
	matches: &[&Expectation],
	lines: usize,
	follow: bool,
	grep: Option<Regex>,
) -> Result<()> {
	let names: Vec<&str> = matches.iter().map(|m| m.name).collect();
	let sources = pm2::log_sources(&names)
		.map_err(|e| miette!("could not locate pm2 log files: {e}"))?;
	if sources.is_empty() {
		bail!(
			"no pm2 log files found for {}; was the deployment saved with `pm2 save`?",
			names.join(", ")
		);
	}
	tail_files(sources, lines, follow, grep)
}

fn tail_files(
	sources: Vec<LogSource>,
	lines: usize,
	follow: bool,
	grep: Option<Regex>,
) -> Result<()> {
	let (tx, rx) = channel::<String>();
	for source in sources {
		let tx = tx.clone();
		let grep = grep.clone();
		thread::spawn(move || tail_one(source, lines, follow, grep.as_ref(), tx));
	}
	drop(tx);

	let stdout = std::io::stdout();
	let mut stdout = stdout.lock();
	while let Ok(msg) = rx.recv() {
		if stdout.write_all(msg.as_bytes()).is_err() {
			break;
		}
	}
	Ok(())
}

fn tail_one(
	source: LogSource,
	lines: usize,
	follow: bool,
	grep: Option<&Regex>,
	tx: Sender<String>,
) {
	let prefix = format!(
		"[{}#{} {}]",
		source.name,
		source
			.pm_id
			.map(|id| id.to_string())
			.unwrap_or_else(|| "?".into()),
		source.stream.as_str()
	);

	if let Ok(initial) = read_last_n_lines(&source.path, lines) {
		for line in initial {
			if grep.is_some_and(|re| !re.is_match(&line)) {
				continue;
			}
			if tx.send(format!("{prefix} {line}\n")).is_err() {
				return;
			}
		}
	}

	if !follow {
		return;
	}

	follow_file(&source.path, &prefix, grep, &tx);
}

fn read_last_n_lines(path: &Path, n: usize) -> std::io::Result<Vec<String>> {
	let contents = std::fs::read_to_string(path)?;
	let lines: Vec<&str> = contents.lines().collect();
	let start = lines.len().saturating_sub(n);
	Ok(lines[start..].iter().map(|s| s.to_string()).collect())
}

fn follow_file(path: &Path, prefix: &str, grep: Option<&Regex>, tx: &Sender<String>) {
	let mut file = match File::open(path) {
		Ok(f) => f,
		Err(_) => return,
	};
	let mut pos = file.seek(SeekFrom::End(0)).unwrap_or(0);
	let mut leftover = String::new();

	loop {
		thread::sleep(Duration::from_millis(500));
		let size = match file.metadata() {
			Ok(m) => m.len(),
			Err(_) => continue,
		};
		if size < pos {
			// Truncated/rotated; start over.
			pos = 0;
			leftover.clear();
			let _ = file.seek(SeekFrom::Start(0));
			continue;
		}
		if size == pos {
			continue;
		}
		let to_read = (size - pos) as usize;
		let mut buf = vec![0u8; to_read];
		if file.seek(SeekFrom::Start(pos)).is_err() || file.read_exact(&mut buf).is_err() {
			continue;
		}
		pos = size;
		let chunk = String::from_utf8_lossy(&buf);
		leftover.push_str(&chunk);
		while let Some(idx) = leftover.find('\n') {
			let line: String = leftover.drain(..=idx).collect();
			let line = line.trim_end_matches('\n').trim_end_matches('\r');
			if grep.is_some_and(|re| !re.is_match(line)) {
				continue;
			}
			if tx.send(format!("{prefix} {line}\n")).is_err() {
				return;
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use crate::actions::tamanu::config::TamanuConfig;

	use super::*;

	fn cfg(facility: bool, fhir_worker: bool) -> TamanuConfig {
		let mut json = serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
			"fhir": { "worker": { "enabled": fhir_worker } },
		});
		if facility {
			json["serverFacilityIds"] = serde_json::json!(["facility-x"]);
		}
		serde_json::from_value(json).unwrap()
	}

	fn names(matched: Vec<&Expectation>) -> Vec<&str> {
		matched.iter().map(|e| e.name).collect()
	}

	#[test]
	fn substring_matches_facility_api_on_systemd() {
		let exps = services::expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg(true, false));
		let m: Vec<&Expectation> = match_name(&exps, "api").collect();
		assert_eq!(names(m), vec!["tamanu-facility-api"]);
	}

	#[test]
	fn substring_matches_central_api_on_systemd() {
		let exps = services::expected(Supervisor::Systemd, ApiServerKind::Central, &cfg(false, false));
		let m: Vec<&Expectation> = match_name(&exps, "api").collect();
		assert_eq!(names(m), vec!["tamanu-central-api"]);
	}

	#[test]
	fn substring_matches_pm2_api() {
		let exps = services::expected(Supervisor::Pm2, ApiServerKind::Facility, &cfg(true, false));
		let m: Vec<&Expectation> = match_name(&exps, "api").collect();
		assert_eq!(names(m), vec!["tamanu-api"]);
	}

	#[test]
	fn fhir_matches_both_workers_on_central() {
		let exps = services::expected(Supervisor::Systemd, ApiServerKind::Central, &cfg(false, true));
		let m: Vec<&Expectation> = match_name(&exps, "fhir").collect();
		assert_eq!(
			names(m),
			vec!["tamanu-central-fhir-resolve", "tamanu-central-fhir-refresh"]
		);
	}

	#[test]
	fn does_not_match_forbidden_facility_singleton() {
		// On systemd, expectations include the forbidden `tamanu-facility` (Down).
		// Logs should skip forbidden services entirely.
		let exps = services::expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg(true, false));
		let m: Vec<&Expectation> = match_name(&exps, "tamanu-facility").collect();
		// The literal `tamanu-facility` (Down) is excluded; matches are
		// substring hits on the kind-prefixed names.
		assert!(
			m.iter().all(|e| e.state == ExpectedState::Up),
			"matched a Down expectation: {m:?}"
		);
		// Ensure at least `tamanu-facility-api` is in there to prove the
		// substring matcher does include legitimate `tamanu-facility-*` units.
		assert!(m.iter().any(|e| e.name == "tamanu-facility-api"));
	}

	#[test]
	fn no_match_returns_empty() {
		let exps = services::expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg(true, false));
		let m: Vec<&Expectation> = match_name(&exps, "nope-no-such-thing").collect();
		assert!(m.is_empty());
	}

	#[test]
	fn read_last_n_lines_returns_trailing_lines() {
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("log.txt");
		std::fs::write(&path, "a\nb\nc\nd\ne\n").unwrap();
		let last = read_last_n_lines(&path, 3).unwrap();
		assert_eq!(last, vec!["c", "d", "e"]);
	}

	#[test]
	fn read_last_n_lines_handles_n_greater_than_file() {
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("log.txt");
		std::fs::write(&path, "only\n").unwrap();
		let last = read_last_n_lines(&path, 10).unwrap();
		assert_eq!(last, vec!["only"]);
	}

	#[test]
	fn read_last_n_lines_empty_file() {
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("log.txt");
		std::fs::write(&path, "").unwrap();
		let last = read_last_n_lines(&path, 10).unwrap();
		assert!(last.is_empty());
	}

	#[test]
	fn pm2_tail_initial_filters_with_grep() {
		// Drive `tail_one` over a fake log file, with grep set, and confirm
		// only matching lines are sent through the channel.
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("tamanu-api-out-1.log");
		std::fs::write(
			&path,
			"alpha\nbeta error: boom\ngamma\ndelta error: kaboom\n",
		)
		.unwrap();
		let source = LogSource {
			name: "tamanu-api".into(),
			pm_id: Some(1),
			stream: crate::actions::tamanu::pm2::LogStream::Out,
			path,
		};
		let re = Regex::new(r"error").unwrap();
		let (tx, rx) = std::sync::mpsc::channel::<String>();
		std::thread::spawn(move || tail_one(source, 10, false, Some(&re), tx));
		let mut received = Vec::new();
		while let Ok(msg) = rx.recv() {
			received.push(msg.trim_end_matches('\n').to_string());
		}
		assert_eq!(received.len(), 2);
		assert!(received[0].contains("beta error: boom"));
		assert!(received[1].contains("delta error: kaboom"));
	}

	#[test]
	fn pm2_tail_initial_no_grep_emits_all() {
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("x.log");
		std::fs::write(&path, "a\nb\nc\n").unwrap();
		let source = LogSource {
			name: "x".into(),
			pm_id: Some(0),
			stream: crate::actions::tamanu::pm2::LogStream::Out,
			path,
		};
		let (tx, rx) = std::sync::mpsc::channel::<String>();
		std::thread::spawn(move || tail_one(source, 10, false, None, tx));
		let mut received = Vec::new();
		while let Ok(msg) = rx.recv() {
			received.push(msg);
		}
		assert_eq!(received.len(), 3);
	}

	#[test]
	fn format_log_line_passes_non_json_through() {
		assert_eq!(
			format_log_line("plain text line", true),
			"plain text line"
		);
	}

	#[test]
	fn format_log_line_no_color_passes_json_through() {
		let line = r#"{"level":"info","msg":"hello"}"#;
		assert_eq!(format_log_line(line, false), line);
	}

	#[test]
	fn format_log_line_colors_json() {
		// We don't lock in the exact ANSI codes — just check that the colored
		// output contains the expected literal pieces and is decorated with
		// escape codes.
		let line = r#"{"level":"info","msg":"hi","status":200}"#;
		let out = format_log_line(line, true);
		assert!(out.contains("\u{1b}["), "expected ANSI escapes in: {out:?}");
		assert!(out.contains("level"));
		assert!(out.contains("info"));
		assert!(out.contains("200"));
	}

	#[test]
	fn format_log_line_handles_malformed_json() {
		let line = "{not really json";
		assert_eq!(format_log_line(line, true), line);
	}

	#[test]
	fn journalctl_pattern_handles_each_instance_kind() {
		let exps = services::expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg(true, false));
		let tasks = exps.iter().find(|e| e.name == "tamanu-facility-tasks").unwrap();
		assert_eq!(journalctl_pattern(tasks), "tamanu-facility-tasks.service");

		let api = exps.iter().find(|e| e.name == "tamanu-facility-api").unwrap();
		assert_eq!(journalctl_pattern(api), "tamanu-facility-api@*.service");

		let frontend = exps.iter().find(|e| e.name == "tamanu-frontend").unwrap();
		assert_eq!(journalctl_pattern(frontend), "tamanu-frontend@*.service");
	}
}
