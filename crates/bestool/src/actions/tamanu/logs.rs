use std::{
	fmt::Write as _,
	fs::File,
	io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
	path::{Path, PathBuf},
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

use bestool_tamanu::{
	ApiServerKind,
	config::load_config,
	pm2::{self, LogSource},
	server_info::query_patient_portal_enabled,
	services::{self, ExpectedState, Expectation, Instances, Supervisor},
};

use crate::actions::{
	Context,
	tamanu::{TamanuArgs, find_tamanu},
};

/// The literal pseudo-service name that triggers caddy log tailing
/// alongside whatever tamanu services are matched.
const CADDY: &str = "caddy";

/// Per-source line formatter used by `tail_one`. Takes the raw line and the
/// caller's "use colours" setting, returns the (possibly transformed) line.
type LineFormatter = fn(&str, bool) -> String;

fn plain_line(line: &str, _use_colours: bool) -> String {
	line.to_string()
}

#[derive(Clone)]
struct TailSource {
	prefix: String,
	path: PathBuf,
	formatter: LineFormatter,
}

/// Tail logs for tamanu services and (optionally) caddy.
///
/// Each NAME is matched as a substring against the expected-Up service
/// list, so `tamanu logs api` picks up `tamanu-{central,facility}-api@*`
/// on systemd and `tamanu-api` on pm2. Multiple names combine: `tamanu
/// logs api fhir` tails both. With no names at all, every expected-Up
/// tamanu service is tailed alongside caddy.
///
/// The literal name `caddy` is recognised as a pseudo-service that
/// tails caddy: from `journalctl -u caddy.service` on Linux, and from
/// `.log` files under `C:\Caddy\logs` (or `C:\Caddy`) on Windows. Caddy
/// emits JSON-per-line logs; bestool detects these and applies
/// opportunistic syntax highlighting per line.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct LogsArgs {
	/// Service names. Each is matched as a substring against the
	/// expected service list. `caddy` is a recognised pseudo-service.
	/// With no names, tails everything (every expected-Up tamanu
	/// service plus caddy).
	pub names: Vec<String>,

	/// Number of trailing lines to print before tailing.
	#[arg(short = 'n', long = "lines", default_value = "10")]
	pub lines: usize,

	/// Follow: keep printing new lines as they arrive. Equivalent to `tail -f`.
	#[arg(short = 'f', long = "follow")]
	pub follow: bool,

	/// Only print lines matching this regex. On Linux this is passed to
	/// `journalctl -g`; on Windows it's applied client-side after reading
	/// from the log files.
	#[arg(short = 'g', long = "grep", value_name = "REGEX")]
	pub grep: Option<Regex>,

	/// Invert the grep match — print lines that do NOT match. Only has an
	/// effect when combined with `--grep`. Mirrors `grep -v`.
	///
	/// `journalctl` has no native inverse-match, so on Linux the filter is
	/// applied client-side when `-v` is in use; without `-v` the regex is
	/// still pushed down into `journalctl -g` for the kernel-side speedup.
	#[arg(short = 'v', long = "invert-match")]
	pub invert: bool,
}

/// Result of partitioning the NAMES argument into tamanu service patterns
/// and the caddy pseudo-service flag.
struct Selection {
	tamanu_names: Vec<String>,
	include_caddy: bool,
	/// Set when the user passed no NAMES at all. Tails every expected-Up
	/// tamanu service. Distinct from `tamanu_names.is_empty()`, which is also
	/// true when the user passed only `caddy` — in that case we must NOT
	/// auto-include every tamanu service.
	all_tamanu: bool,
}

fn select(names: &[String]) -> Selection {
	if names.is_empty() {
		return Selection {
			tamanu_names: Vec::new(),
			include_caddy: true,
			all_tamanu: true,
		};
	}
	let mut tamanu_names = Vec::new();
	let mut include_caddy = false;
	for n in names {
		if n == CADDY {
			include_caddy = true;
		} else {
			tamanu_names.push(n.clone());
		}
	}
	Selection {
		tamanu_names,
		include_caddy,
		all_tamanu: false,
	}
}

pub async fn run(args: LogsArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>();
	let selection = select(&args.names);

	let (_, root) = find_tamanu(tamanu)?;
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

	// Read `features.patientPortal` from the DB so that `bestool tamanu logs
	// tamanu-patientportal` actually matches when the flag is on; without
	// this lookup the portal expectation is `Down` and gets filtered out by
	// the `state == Up` predicate below.
	let patient_portal_enabled =
		if matches!(supervisor, Supervisor::Systemd) && matches!(kind, ApiServerKind::Central) {
			match bestool_postgres::pool::connect_one(
				&config.database_url(),
				"bestool-tamanu-logs",
			)
			.await
			{
				Ok(client) => query_patient_portal_enabled(&client).await,
				Err(err) => {
					tracing::warn!(%err, "could not query features.patientPortal; assuming false");
					false
				}
			}
		} else {
			false
		};

	let all_expectations = services::expected(supervisor, kind, &config, patient_portal_enabled);
	let up_expectations: Vec<&Expectation> = all_expectations
		.iter()
		.filter(|e| e.state == ExpectedState::Up)
		.collect();

	// We use the same matcher as the lifecycle commands but only consider
	// expected-Up services as candidates for the NAMES filter. When the user
	// passed no NAMES we tail every up service; when they passed only `caddy`
	// we tail no tamanu services at all (just caddy below).
	let matches: Vec<&Expectation> = if selection.all_tamanu {
		up_expectations.clone()
	} else if selection.tamanu_names.is_empty() {
		Vec::new()
	} else {
		let tamanu_name_refs: Vec<&str> = selection
			.tamanu_names
			.iter()
			.map(String::as_str)
			.collect();
		let up_owned: Vec<Expectation> = up_expectations.iter().map(|e| (*e).clone()).collect();
		let m = services::match_names(&up_owned, &tamanu_name_refs)?;
		m.iter()
			.map(|e| {
				up_expectations
					.iter()
					.copied()
					.find(|x| x.name == e.name)
					.expect("match came from up_expectations")
			})
			.collect()
	};

	debug!(
		matched = ?matches.iter().map(|m| m.name).collect::<Vec<_>>(),
		caddy = selection.include_caddy,
		"logs selection"
	);

	let grep = args.grep.map(|re| GrepFilter {
		regex: re,
		invert: args.invert,
	});

	match supervisor {
		Supervisor::Systemd => run_journalctl(
			&matches,
			selection.include_caddy,
			args.lines,
			args.follow,
			grep.as_ref(),
			tamanu.use_colours,
		),
		Supervisor::Pm2 => run_pm2_logs(
			&matches,
			selection.include_caddy,
			args.lines,
			args.follow,
			grep,
			tamanu.use_colours,
		),
	}
}

/// A compiled regex paired with an inversion flag, so the rest of the code
/// has one place to ask "does this line pass the filter?".
#[derive(Clone)]
struct GrepFilter {
	regex: Regex,
	invert: bool,
}

impl GrepFilter {
	fn matches(&self, line: &str) -> bool {
		self.regex.is_match(line) ^ self.invert
	}
}

/// Windows: caddy runs out of `C:\Caddy`, with log files typically in
/// `C:\Caddy\logs\*.log`. We probe that directory first, then the install
/// root, and tail every `.log` we find.
fn caddy_tail_sources_windows() -> Result<Vec<TailSource>> {
	let files = caddy_log_files_windows()?;
	debug!(count = files.len(), "found caddy log files");
	Ok(files
		.into_iter()
		.map(|path| TailSource {
			prefix: caddy_prefix(&path),
			path,
			formatter: format_log_line,
		})
		.collect())
}

fn caddy_log_files_windows() -> Result<Vec<PathBuf>> {
	const ROOTS: &[&str] = &[r"C:\Caddy\logs", r"C:\Caddy"];
	for root in ROOTS {
		let entries = match std::fs::read_dir(root) {
			Ok(e) => e,
			Err(_) => continue,
		};
		let mut files: Vec<PathBuf> = entries
			.filter_map(|e| e.ok())
			.map(|e| e.path())
			.filter(|p| p.is_file())
			.filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("log")))
			.collect();
		if !files.is_empty() {
			files.sort();
			return Ok(files);
		}
	}
	bail!(
		"no caddy log files found under {} or {}",
		ROOTS[0],
		ROOTS[1]
	)
}

fn caddy_prefix(path: &Path) -> String {
	let name = path
		.file_name()
		.and_then(|s| s.to_str())
		.unwrap_or("caddy");
	format!("[{name}]")
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

/// Single journalctl call covering every matched tamanu unit plus,
/// optionally, caddy. Caddy emits JSON-per-line logs; on systemd we run
/// with `--output=cat` so journalctl's own prefix doesn't double up
/// with caddy's timestamps, and pipe stdout through the JSON
/// highlighter (which is opportunistic — non-JSON lines pass through
/// unchanged).
fn run_journalctl(
	matches: &[&Expectation],
	include_caddy: bool,
	lines: usize,
	follow: bool,
	grep: Option<&GrepFilter>,
	use_colours: bool,
) -> Result<()> {
	if matches.is_empty() && !include_caddy {
		bail!("nothing to tail: no matched services and caddy not included");
	}

	let mut cmd = Command::new("journalctl");
	for m in matches {
		cmd.arg("-u").arg(journalctl_pattern(m));
	}
	if include_caddy {
		cmd.arg("-u").arg("caddy.service");
	}
	cmd.arg("-n").arg(lines.to_string());
	if follow {
		cmd.arg("-f");
	}
	// journalctl has no inverse-match flag, so when `-v` is in play we have to
	// pull the unfiltered stream and apply the inverted regex client-side.
	// Without `-v` we still let journalctl do the work (kernel-side scan).
	if let Some(g) = grep
		&& !g.invert
	{
		cmd.arg("-g").arg(g.regex.as_str());
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
		
		if let Some(g) = grep
			&& g.invert
			&& !g.matches(&line)
		{
      // In `-v` mode, journalctl was given no `-g`, so apply the inverted
		  // match here. Non-inverted matches were already filtered by journalctl.
      continue;
    } else if line.trim().is_empty() {
      // `journalctl --output=cat` emits the raw MESSAGE field, which is empty
      // for some service log records (e.g. heartbeat / no-op messages) and
      // also gets emitted as a blank line when journald separators slip
      // through. Either way they're noise — skip them.
			continue;
		}
    
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
	include_caddy: bool,
	lines: usize,
	follow: bool,
	grep: Option<GrepFilter>,
	use_colours: bool,
) -> Result<()> {
	let mut tail_sources: Vec<TailSource> = Vec::new();

	if !matches.is_empty() {
		let names: Vec<&str> = matches.iter().map(|m| m.name).collect();
		let sources = pm2::log_sources(&names)
			.map_err(|e| miette!("could not locate pm2 log files: {e}"))?;
		if sources.is_empty() {
			bail!(
				"no pm2 log files found for {}; was the deployment saved with `pm2 save`?",
				names.join(", ")
			);
		}
		tail_sources.extend(sources.into_iter().map(pm2_log_to_tail));
	}

	if include_caddy {
		tail_sources.extend(caddy_tail_sources_windows()?);
	}

	if tail_sources.is_empty() {
		bail!("nothing to tail: no matched services and caddy not included");
	}

	tail_files(tail_sources, lines, follow, grep, use_colours)
}

fn pm2_log_to_tail(source: LogSource) -> TailSource {
	let prefix = format!(
		"[{}#{} {}]",
		source.name,
		source
			.pm_id
			.map(|id| id.to_string())
			.unwrap_or_else(|| "?".into()),
		source.stream.as_str()
	);
	TailSource {
		prefix,
		path: source.path,
		formatter: plain_line,
	}
}

fn tail_files(
	sources: Vec<TailSource>,
	lines: usize,
	follow: bool,
	grep: Option<GrepFilter>,
	use_colours: bool,
) -> Result<()> {
	let (tx, rx) = channel::<String>();
	for source in sources {
		let tx = tx.clone();
		let grep = grep.clone();
		thread::spawn(move || tail_one(source, lines, follow, grep.as_ref(), use_colours, tx));
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
	source: TailSource,
	lines: usize,
	follow: bool,
	grep: Option<&GrepFilter>,
	use_colours: bool,
	tx: Sender<String>,
) {
	let TailSource {
		prefix,
		path,
		formatter,
	} = source;

	if let Ok(initial) = read_last_n_lines(&path, lines) {
		for line in initial {
			if grep.is_some_and(|g| !g.matches(&line)) {
				continue;
			}
			let formatted = formatter(&line, use_colours);
			if tx.send(format!("{prefix} {formatted}\n")).is_err() {
				return;
			}
		}
	}

	if !follow {
		return;
	}

	follow_file(&path, &prefix, grep, formatter, use_colours, &tx);
}

/// Read the last `n` lines of `path` without loading the whole file.
///
/// pm2 log files on Windows can grow to many gigabytes (Tamanu services log
/// heavily and rotation is operator-managed). The previous implementation
/// `std::fs::read_to_string`-d the entire file before slicing the tail,
/// which lets a multi-GB log file pin all of bestool's memory and freeze the
/// host — exactly the pathological state operators were hitting.
///
/// This version seeks to the end and reads backward in fixed-size chunks
/// until it's accumulated more than `n` newlines (or hit the start of the
/// file). The kept buffer is therefore bounded by roughly `n × avg-line-len +
/// CHUNK`, independent of total file size.
fn read_last_n_lines(path: &Path, n: usize) -> std::io::Result<Vec<String>> {
	if n == 0 {
		return Ok(Vec::new());
	}

	let mut file = File::open(path)?;
	let mut pos = file.seek(SeekFrom::End(0))?;
	if pos == 0 {
		return Ok(Vec::new());
	}

	const CHUNK: u64 = 8 * 1024;
	let mut buf: Vec<u8> = Vec::new();

	while pos > 0 {
		let to_read = CHUNK.min(pos);
		pos -= to_read;
		file.seek(SeekFrom::Start(pos))?;
		let mut chunk = vec![0u8; to_read as usize];
		file.read_exact(&mut chunk)?;
		chunk.extend_from_slice(&buf);
		buf = chunk;
		// Stop once we've crossed the n-th line boundary from the end. We need
		// *more than* `n` newlines so the first kept line is bounded on the
		// left by a real newline (rather than possibly being mid-line).
		if buf.iter().filter(|&&b| b == b'\n').count() > n {
			break;
		}
	}

	// `from_utf8_lossy` keeps us safe if a backward chunk boundary split a
	// multi-byte UTF-8 codepoint — corrupted byte sequences become `U+FFFD`
	// rather than panicking. Log files we tail are normally ASCII anyway.
	let text = String::from_utf8_lossy(&buf);
	let lines: Vec<&str> = text.lines().collect();
	let start = lines.len().saturating_sub(n);
	Ok(lines[start..].iter().map(|s| s.to_string()).collect())
}

fn follow_file(
	path: &Path,
	prefix: &str,
	grep: Option<&GrepFilter>,
	formatter: LineFormatter,
	use_colours: bool,
	tx: &Sender<String>,
) {
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
		// Cap how much we pull in one iteration. After a rotation reset
		// (`size < pos` above) the next tick can otherwise allocate hundreds
		// of MB at once trying to consume the whole new file in a single
		// read — same class of OOM as `read_last_n_lines` used to hit.
		const MAX_PER_ITER: u64 = 4 * 1024 * 1024;
		let to_read = (size - pos).min(MAX_PER_ITER) as usize;
		let mut buf = vec![0u8; to_read];
		if file.seek(SeekFrom::Start(pos)).is_err() || file.read_exact(&mut buf).is_err() {
			continue;
		}
		pos += to_read as u64;
		let chunk = String::from_utf8_lossy(&buf);
		leftover.push_str(&chunk);
		while let Some(idx) = leftover.find('\n') {
			let line: String = leftover.drain(..=idx).collect();
			let line = line.trim_end_matches('\n').trim_end_matches('\r');
			if grep.is_some_and(|g| !g.matches(line)) {
				continue;
			}
			let formatted = formatter(line, use_colours);
			if tx.send(format!("{prefix} {formatted}\n")).is_err() {
				return;
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use bestool_tamanu::config::TamanuConfig;

	use super::*;

	fn cfg(facility: bool, fhir_worker: bool) -> TamanuConfig {
		let mut json = serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
			"integrations": { "fhir": { "worker": { "enabled": fhir_worker } } },
		});
		if facility {
			json["serverFacilityIds"] = serde_json::json!(["facility-x"]);
		}
		serde_json::from_value(json).unwrap()
	}

	#[test]
	fn select_empty_includes_caddy_and_all_tamanu() {
		let s = select(&[]);
		assert!(s.include_caddy);
		assert!(s.all_tamanu);
		assert!(s.tamanu_names.is_empty());
	}

	#[test]
	fn select_caddy_alone_does_not_imply_all_tamanu() {
		// Regression: `bestool tamanu logs caddy` used to tail every service
		// because empty `tamanu_names` was conflated with "no filter at all".
		let s = select(&["caddy".to_string()]);
		assert!(s.include_caddy);
		assert!(!s.all_tamanu);
		assert!(s.tamanu_names.is_empty());
	}

	#[test]
	fn select_caddy_with_others() {
		let s = select(&["caddy".to_string(), "api".to_string()]);
		assert!(s.include_caddy);
		assert!(!s.all_tamanu);
		assert_eq!(s.tamanu_names, vec!["api".to_string()]);
	}

	#[test]
	fn select_without_caddy_excludes_it() {
		let s = select(&["api".to_string(), "tasks".to_string()]);
		assert!(!s.include_caddy);
		assert!(!s.all_tamanu);
		assert_eq!(s.tamanu_names, vec!["api".to_string(), "tasks".to_string()]);
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
	fn grep_filter_inverts_when_invert_set() {
		let g = GrepFilter {
			regex: Regex::new(r"error").unwrap(),
			invert: true,
		};
		assert!(!g.matches("an error happened"));
		assert!(g.matches("an info line"));
	}

	#[test]
	fn grep_filter_matches_normally_when_not_inverted() {
		let g = GrepFilter {
			regex: Regex::new(r"error").unwrap(),
			invert: false,
		};
		assert!(g.matches("an error happened"));
		assert!(!g.matches("an info line"));
	}

	#[test]
	fn pm2_tail_invert_grep_emits_non_matches() {
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("invert.log");
		std::fs::write(
			&path,
			"alpha info: ok\nbeta error: boom\ngamma info: ok\ndelta error: kaboom\n",
		)
		.unwrap();
		let source = pm2_log_to_tail(LogSource {
			name: "tamanu-api".into(),
			pm_id: Some(1),
			stream: pm2::LogStream::Out,
			path,
		});
		let g = GrepFilter {
			regex: Regex::new(r"error").unwrap(),
			invert: true,
		};
		let (tx, rx) = std::sync::mpsc::channel::<String>();
		std::thread::spawn(move || tail_one(source, 10, false, Some(&g), false, tx));
		let mut received = Vec::new();
		while let Ok(msg) = rx.recv() {
			received.push(msg.trim_end_matches('\n').to_string());
		}
		assert_eq!(received.len(), 2);
		assert!(received[0].contains("alpha info"));
		assert!(received[1].contains("gamma info"));
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
	fn read_last_n_lines_zero_returns_empty_without_reading() {
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("log.txt");
		std::fs::write(&path, "a\nb\nc\n").unwrap();
		assert!(read_last_n_lines(&path, 0).unwrap().is_empty());
	}

	#[test]
	fn read_last_n_lines_no_trailing_newline() {
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("log.txt");
		std::fs::write(&path, "a\nb\nc").unwrap();
		assert_eq!(read_last_n_lines(&path, 2).unwrap(), vec!["b", "c"]);
	}

	#[test]
	fn read_last_n_lines_handles_file_much_larger_than_request() {
		// The whole point of the rewrite: we must not load the entire file
		// to return the trailing handful of lines. Build a file that's well
		// over the backward-read chunk size (8 KiB) and check we still get a
		// tight, correct tail.
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("big.log");
		let mut contents = String::with_capacity(64 * 1024);
		for i in 0..4096 {
			contents.push_str(&format!("line-{i}\n"));
		}
		std::fs::write(&path, &contents).unwrap();
		let last = read_last_n_lines(&path, 5).unwrap();
		assert_eq!(
			last,
			vec!["line-4091", "line-4092", "line-4093", "line-4094", "line-4095"]
		);
	}

	#[test]
	fn read_last_n_lines_handles_n_spanning_multiple_backward_chunks() {
		// Force the backward-read loop to iterate more than once.
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("big.log");
		// Each line is 50 bytes; 1000 lines = ~50 KiB, asking for 800 lines
		// requires reading back more than one 8 KiB chunk.
		let mut contents = String::new();
		for i in 0..1000 {
			contents.push_str(&format!("{:>48}\n", i));
		}
		std::fs::write(&path, &contents).unwrap();
		let last = read_last_n_lines(&path, 800).unwrap();
		assert_eq!(last.len(), 800);
		assert_eq!(last.first().unwrap().trim(), "200");
		assert_eq!(last.last().unwrap().trim(), "999");
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
		let source = pm2_log_to_tail(LogSource {
			name: "tamanu-api".into(),
			pm_id: Some(1),
			stream: pm2::LogStream::Out,
			path,
		});
		let g = GrepFilter {
			regex: Regex::new(r"error").unwrap(),
			invert: false,
		};
		let (tx, rx) = std::sync::mpsc::channel::<String>();
		std::thread::spawn(move || tail_one(source, 10, false, Some(&g), false, tx));
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
		let source = pm2_log_to_tail(LogSource {
			name: "x".into(),
			pm_id: Some(0),
			stream: pm2::LogStream::Out,
			path,
		});
		let (tx, rx) = std::sync::mpsc::channel::<String>();
		std::thread::spawn(move || tail_one(source, 10, false, None, false, tx));
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
	fn caddy_prefix_uses_filename() {
		let mut p = PathBuf::from("logs");
		p.push("access.log");
		assert_eq!(caddy_prefix(&p), "[access.log]");
	}

	#[test]
	fn journalctl_pattern_handles_each_instance_kind() {
		let exps = services::expected(Supervisor::Systemd, ApiServerKind::Facility, &cfg(true, false), false);
		let tasks = exps.iter().find(|e| e.name == "tamanu-facility-tasks").unwrap();
		assert_eq!(journalctl_pattern(tasks), "tamanu-facility-tasks.service");

		let api = exps.iter().find(|e| e.name == "tamanu-facility-api").unwrap();
		assert_eq!(journalctl_pattern(api), "tamanu-facility-api@*.service");

		let frontend = exps.iter().find(|e| e.name == "tamanu-frontend").unwrap();
		assert_eq!(journalctl_pattern(frontend), "tamanu-frontend@*.service");
	}
}
