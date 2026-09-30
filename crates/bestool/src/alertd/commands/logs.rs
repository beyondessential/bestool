//! `bestool alertd logs`: the daemon's log entries, read from where its service
//! writes them rather than asked of the daemon, so they're available while it's
//! down or crash-looping.

use std::io::{IsTerminal as _, Write};

use clap::Parser;
#[cfg(any(windows, target_os = "linux", test))]
use jiff::Timestamp;
use miette::{IntoDiagnostic as _, Result};
use owo_colors::OwoColorize as _;
#[cfg(any(windows, target_os = "linux", test))]
use serde_json::Value;

/// How often a follow checks for new entries.
#[cfg(windows)]
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Debug, Clone, Parser)]
pub struct LogsArgs {
	/// How many recent entries to print before following
	#[arg(short = 'n', long, default_value_t = 50)]
	lines: usize,

	/// Print the recent entries and exit, instead of following new ones
	#[arg(long)]
	no_follow: bool,
}

/// spec: ALOG
pub async fn show_logs(args: LogsArgs) -> Result<()> {
	let style = Style::detect();
	tokio::task::spawn_blocking(move || show_platform_logs(&args, style))
		.await
		.into_diagnostic()?
}

/// Terminal styling, matching the daemon's own coloured output.
#[derive(Debug, Clone, Copy)]
struct Style {
	colour: bool,
}

impl Style {
	fn detect() -> Self {
		Self {
			colour: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
		}
	}

	fn dim(self, text: &str) -> String {
		if self.colour {
			text.dimmed().to_string()
		} else {
			text.to_owned()
		}
	}

	#[cfg(any(windows, test))]
	fn bold(self, text: &str) -> String {
		if self.colour {
			text.bold().to_string()
		} else {
			text.to_owned()
		}
	}

	#[cfg(any(windows, test))]
	fn italic(self, text: &str) -> String {
		if self.colour {
			text.italic().to_string()
		} else {
			text.to_owned()
		}
	}

	/// The level, right-aligned to the width of the longest, as the daemon
	/// prints it.
	#[cfg(any(windows, test))]
	fn level(self, level: &str) -> String {
		let padded = format!("{level:>5}");
		if !self.colour {
			return padded;
		}
		match level {
			"TRACE" => padded.purple().to_string(),
			"DEBUG" => padded.blue().to_string(),
			"INFO" => padded.green().to_string(),
			"WARN" => padded.yellow().to_string(),
			"ERROR" => padded.red().to_string(),
			_ => padded,
		}
	}
}

/// A printable entry, and the instant it was written when that's known.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
	at: Option<Timestamp>,
	text: String,
}

/// Write each entry on its own line. Returns false once the reader has gone
/// away (e.g. a pipe into a filter that exited), which ends the command.
fn print_lines<'a>(out: &mut impl Write, lines: impl IntoIterator<Item = &'a str>) -> bool {
	for line in lines {
		if writeln!(out, "{line}").is_err() {
			return false;
		}
	}
	out.flush().is_ok()
}

/// Render one line of the service's JSON log into the form the daemon prints
/// to a terminal: timestamp, level, spans, target, message, then the remaining
/// fields. `None` when the line isn't a log entry, e.g. one still being written.
///
/// spec: ALOG#output
#[cfg(any(windows, test))]
fn render_json_entry(line: &str, style: Style) -> Option<(Timestamp, String)> {
	let Value::Object(entry) = serde_json::from_str(line).ok()? else {
		return None;
	};
	let timestamp = entry.get("timestamp")?.as_str()?;
	let at: Timestamp = timestamp.parse().ok()?;
	let level = entry.get("level").and_then(Value::as_str).unwrap_or("?");
	let no_fields = serde_json::Map::new();
	let fields = entry
		.get("fields")
		.and_then(Value::as_object)
		.unwrap_or(&no_fields);
	// Events bridged from the `log` crate carry their real target in a field,
	// which the terminal formatter shows in place of the bridge's own target.
	let target = fields
		.get("log.target")
		.or_else(|| entry.get("target"))
		.and_then(Value::as_str);

	let mut out = format!("{} {} ", style.dim(timestamp), style.level(level));

	// `spans` lists every span the event was in, outermost first; `span` is
	// only the innermost, so it's the fallback when the list wasn't recorded.
	let spans: Vec<&Value> = match entry.get("spans") {
		Some(Value::Array(spans)) => spans.iter().collect(),
		_ => entry.get("span").into_iter().collect(),
	};
	let mut in_span = false;
	for span in spans.iter().filter_map(|span| span.as_object()) {
		let Some(name) = span.get("name").and_then(Value::as_str) else {
			continue;
		};
		if in_span {
			out.push(':');
		}
		out.push_str(&style.bold(name));
		let span_fields: Vec<String> = span
			.iter()
			.filter(|(key, _)| *key != "name")
			.map(|(key, value)| field_text(key, value, style))
			.collect();
		if !span_fields.is_empty() {
			out.push_str(&style.bold("{"));
			out.push_str(&span_fields.join(" "));
			out.push_str(&style.bold("}"));
		}
		in_span = true;
	}
	if in_span {
		out.push_str(": ");
	}

	if let Some(target) = target {
		out.push_str(&style.dim(&format!("{target}:")));
		out.push(' ');
	}

	let mut parts = Vec::new();
	if let Some(message) = fields.get("message") {
		parts.push(value_text(message));
	}
	parts.extend(
		fields
			.iter()
			.filter(|(key, _)| *key != "message" && !key.starts_with("log."))
			.map(|(key, value)| field_text(key, value, style)),
	);
	out.push_str(&parts.join(" "));

	Some((at, out))
}

#[cfg(any(windows, test))]
fn field_text(key: &str, value: &Value, style: Style) -> String {
	format!(
		"{}{}{}",
		style.italic(key),
		style.dim("="),
		value_text(value)
	)
}

#[cfg(any(windows, test))]
fn value_text(value: &Value) -> String {
	match value {
		Value::String(text) => text.clone(),
		other => other.to_string(),
	}
}

/// Render one record of `journalctl --output=json` as the daemon prints to a
/// terminal. The daemon logs to the journal without timestamps, since the
/// journal records its own, so the record's timestamp is put back in front of
/// the message in the same form the daemon writes. `None` for a record that
/// isn't a log line.
///
/// spec: ALOG#output
#[cfg(any(target_os = "linux", test))]
fn render_journal_record(line: &str, style: Style) -> Option<String> {
	let Value::Object(record) = serde_json::from_str(line).ok()? else {
		return None;
	};
	let micros: i64 = record.get("__REALTIME_TIMESTAMP")?.as_str()?.parse().ok()?;
	let at = Timestamp::from_microsecond(micros).ok()?;
	// The journal hands a message that isn't valid UTF-8 over as its bytes.
	let message = match record.get("MESSAGE")? {
		Value::String(message) => message.clone(),
		Value::Array(bytes) => {
			let bytes: Vec<u8> = bytes
				.iter()
				.filter_map(|byte| byte.as_u64().and_then(|b| u8::try_from(b).ok()))
				.collect();
			String::from_utf8_lossy(&bytes).into_owned()
		}
		_ => return None,
	};
	Some(format!("{} {message}", style.dim(&format!("{at:.6}"))))
}

#[cfg(windows)]
fn show_platform_logs(args: &LogsArgs, style: Style) -> Result<()> {
	let mut dir = files::LogDir::new(crate::alertd::windows_service::service_log_dir(), style);
	let mut out = std::io::stdout().lock();

	let recent = dir.recent(args.lines)?;
	if !print_lines(&mut out, recent.iter().map(|entry| entry.text.as_str())) || args.no_follow {
		return Ok(());
	}

	// spec: ALOG#options
	loop {
		std::thread::sleep(POLL_INTERVAL);
		let entries = dir.poll()?;
		if !print_lines(&mut out, entries.iter().map(|entry| entry.text.as_str())) {
			return Ok(());
		}
	}
}

#[cfg(target_os = "linux")]
fn show_platform_logs(args: &LogsArgs, style: Style) -> Result<()> {
	journal::show(args, style)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn show_platform_logs(_args: &LogsArgs, _style: Style) -> Result<()> {
	miette::bail!("`bestool alertd logs` is only supported on Windows and Linux")
}

/// The daemon's journal on Linux.
///
/// spec: ALOG#log-sources
#[cfg(target_os = "linux")]
mod journal {
	use std::{
		io::{BufRead as _, BufReader},
		process::{Command, Stdio},
	};

	use miette::{IntoDiagnostic as _, Result, bail, miette};
	use tracing::info;

	use super::{LogsArgs, Style, print_lines, render_journal_record};

	/// The unit's `SyslogIdentifier`, which sets the daemon's entries apart
	/// from every other bestool invocation's.
	const IDENTIFIER: &str = "bestool-alertd";

	pub(super) fn show(args: &LogsArgs, style: Style) -> Result<()> {
		// Without root (or journal group membership) journalctl finds no
		// entries for a system service and still succeeds, which would look
		// like a daemon that never logged.
		ensure_root_or_reexec()?;
		ensure_any_entries()?;

		let mut cmd = Command::new("journalctl");
		cmd.args(["--quiet", "--output=json", "--identifier", IDENTIFIER])
			.arg("--lines")
			.arg(args.lines.to_string());
		if !args.no_follow {
			cmd.arg("--follow");
		}
		let mut child = cmd.stdout(Stdio::piped()).spawn().into_diagnostic()?;
		let stdout = child
			.stdout
			.take()
			.ok_or_else(|| miette!("no stdout from journalctl"))?;

		let mut out = std::io::stdout().lock();
		for line in BufReader::new(stdout).lines() {
			let Ok(line) = line else { break };
			let Some(text) = render_journal_record(&line, style) else {
				continue;
			};
			if !print_lines(&mut out, [text.as_str()]) {
				let _ = child.kill();
				return Ok(());
			}
		}

		let status = child.wait().into_diagnostic()?;
		if !status.success() {
			bail!("journalctl exited with {status}");
		}
		Ok(())
	}

	fn ensure_any_entries() -> Result<()> {
		let probe = Command::new("journalctl")
			.args([
				"--quiet",
				"--output=json",
				"--lines=1",
				"--identifier",
				IDENTIFIER,
			])
			.output()
			.into_diagnostic()?;
		if !probe.status.success() {
			bail!(
				"journalctl exited with {}: {}",
				probe.status,
				String::from_utf8_lossy(&probe.stderr).trim()
			);
		}
		if probe.stdout.iter().all(u8::is_ascii_whitespace) {
			bail!("no alert daemon logs found in the journal under the {IDENTIFIER} identifier");
		}
		Ok(())
	}

	fn ensure_root_or_reexec() -> Result<()> {
		if privilege::user::privileged() {
			return Ok(());
		}
		info!("not running as root; re-executing under sudo to read the journal");
		let status = Command::new("sudo")
			.args(std::env::args())
			.status()
			.into_diagnostic()?;
		std::process::exit(status.code().unwrap_or(1));
	}
}

/// The service's log files on Windows.
///
/// The daemon starts a new series of files each time it starts, rotated daily,
/// so the directory holds several files written at overlapping times. Each is
/// read on its own and the entries merged by timestamp.
///
/// spec: ALOG#log-sources
#[cfg(any(windows, test))]
mod files {
	use std::{
		collections::BTreeMap,
		fs::File,
		io::{self, Read as _, Seek as _, SeekFrom},
		path::{Path, PathBuf},
	};

	use jiff::Timestamp;
	use miette::{Result, miette};

	use super::{Entry, Style, render_json_entry};

	/// The most read from a file in one go, so a large backlog in a file that
	/// appears mid-follow is taken in steps rather than one huge allocation.
	const MAX_READ: u64 = 4 * 1024 * 1024;

	/// Where reading a file has got to.
	#[derive(Debug, Default)]
	struct Cursor {
		/// Just past the file's last complete line: a line still being written
		/// is left for the next read.
		pos: u64,
		/// The timestamp of the last entry read, which a line that isn't an
		/// entry is ordered by, so it stays after the entry it followed.
		last_at: Option<Timestamp>,
	}

	impl Cursor {
		fn entry(&mut self, line: &str, style: Style) -> Entry {
			match render_json_entry(line, style) {
				Some((at, text)) => {
					self.last_at = Some(at);
					Entry { at: Some(at), text }
				}
				None => Entry {
					at: self.last_at,
					text: line.to_owned(),
				},
			}
		}
	}

	pub(super) struct LogDir {
		dir: PathBuf,
		style: Style,
		cursors: BTreeMap<PathBuf, Cursor>,
	}

	impl LogDir {
		pub(super) fn new(dir: PathBuf, style: Style) -> Self {
			Self {
				dir,
				style,
				cursors: BTreeMap::new(),
			}
		}

		fn list(&self) -> io::Result<Vec<PathBuf>> {
			let mut files = Vec::new();
			for entry in std::fs::read_dir(&self.dir)? {
				let entry = entry?;
				if entry.file_type()?.is_file() {
					files.push(entry.path());
				}
			}
			files.sort();
			Ok(files)
		}

		/// The most recent `n` entries across every file, oldest first. Each
		/// file is then followed from where this left it.
		pub(super) fn recent(&mut self, n: usize) -> Result<Vec<Entry>> {
			let files = match self.list() {
				Ok(files) => files,
				Err(err) if err.kind() == io::ErrorKind::NotFound => {
					return Err(miette!(
						"no alert daemon logs found: {} does not exist",
						self.dir.display()
					));
				}
				Err(err) => return Err(unreadable(&self.dir, &err)),
			};
			if files.is_empty() {
				return Err(miette!(
					"no alert daemon logs found in {}",
					self.dir.display()
				));
			}

			// Any entry among the last `n` overall is among the last `n` of its
			// own file, so reading that many from each is enough.
			let mut entries = Vec::new();
			for path in files {
				let (lines, pos) = read_tail(&path, n).map_err(|err| unreadable(&path, &err))?;
				let mut cursor = Cursor { pos, last_at: None };
				entries.extend(lines.iter().map(|line| cursor.entry(line, self.style)));
				self.cursors.insert(path, cursor);
			}

			// Stable, so lines of one file with the same timestamp keep their order.
			entries.sort_by_key(|entry| entry.at);
			let start = entries.len().saturating_sub(n);
			Ok(entries.split_off(start))
		}

		/// The entries written since the last call, oldest first: new lines in
		/// the files already known, and every line of a file that has appeared
		/// since, as the next day's file or a restarted daemon's new series.
		/// Files that have been deleted are forgotten.
		///
		/// spec: ALOG#options
		pub(super) fn poll(&mut self) -> Result<Vec<Entry>> {
			// The directory being briefly unlistable isn't worth ending a follow
			// over; the next poll tries again.
			let Ok(files) = self.list() else {
				return Ok(Vec::new());
			};
			self.cursors.retain(|path, _| files.contains(path));

			let style = self.style;
			let mut entries = Vec::new();
			for path in files {
				let cursor = self.cursors.entry(path.clone()).or_default();
				match read_from(&path, cursor.pos) {
					Ok((lines, pos)) => {
						cursor.pos = pos;
						entries.extend(lines.iter().map(|line| cursor.entry(line, style)));
					}
					// Deleted between listing and reading.
					Err(err) if err.kind() == io::ErrorKind::NotFound => {}
					Err(err) => return Err(unreadable(&path, &err)),
				}
			}

			entries.sort_by_key(|entry| entry.at);
			Ok(entries)
		}
	}

	/// spec: ALOG#log-sources
	fn unreadable(path: &Path, err: &io::Error) -> miette::Report {
		if err.kind() == io::ErrorKind::PermissionDenied {
			miette!(
				"cannot read {}: {err}\nreading the alert daemon's logs requires Administrator privileges (open an elevated shell)",
				path.display()
			)
		} else {
			miette!("cannot read {}: {err}", path.display())
		}
	}

	/// The complete, non-blank lines of `bytes`, and how many bytes they span:
	/// a trailing line without its newline is still being written.
	fn complete_lines(bytes: &[u8]) -> (Vec<String>, usize) {
		let complete = bytes
			.iter()
			.rposition(|&b| b == b'\n')
			.map_or(0, |idx| idx + 1);
		let lines = String::from_utf8_lossy(&bytes[..complete])
			.lines()
			.filter(|line| !line.trim().is_empty())
			.map(str::to_owned)
			.collect();
		(lines, complete)
	}

	/// The last `n` complete lines of `path` without reading the whole file,
	/// and the offset just past them.
	fn read_tail(path: &Path, n: usize) -> io::Result<(Vec<String>, u64)> {
		const CHUNK: u64 = 8 * 1024;

		let mut file = File::open(path)?;
		let len = file.metadata()?.len();

		let mut start = len;
		let mut buf: Vec<u8> = Vec::new();
		while start > 0 {
			let size = CHUNK.min(start);
			start -= size;
			file.seek(SeekFrom::Start(start))?;
			let mut chunk = vec![0; size as usize];
			file.read_exact(&mut chunk)?;
			chunk.extend_from_slice(&buf);
			buf = chunk;
			// More than `n` newlines means the first of the kept lines starts
			// on a real line boundary, rather than partway through a line.
			if buf.iter().filter(|&&b| b == b'\n').count() > n {
				break;
			}
		}

		let (mut lines, complete) = complete_lines(&buf);
		let skip = lines.len().saturating_sub(n);
		Ok((lines.split_off(skip), start + complete as u64))
	}

	/// The complete lines of `path` from `pos` on, and the offset just past them.
	fn read_from(path: &Path, pos: u64) -> io::Result<(Vec<String>, u64)> {
		let mut file = File::open(path)?;
		let len = file.metadata()?.len();
		// Shorter than where we got to: it was emptied, so start it again.
		let pos = if len < pos { 0 } else { pos };
		if len == pos {
			return Ok((Vec::new(), pos));
		}

		let size = (len - pos).min(MAX_READ);
		file.seek(SeekFrom::Start(pos))?;
		let mut buf = vec![0; size as usize];
		file.read_exact(&mut buf)?;

		let (lines, complete) = complete_lines(&buf);
		if complete == 0 && size == MAX_READ {
			// A single line longer than a whole read would otherwise never
			// complete; take it as it is.
			let line = String::from_utf8_lossy(&buf).into_owned();
			return Ok((vec![line], pos + size));
		}
		Ok((lines, pos + complete as u64))
	}

	#[cfg(test)]
	mod tests {
		use std::io::Write as _;

		use tempfile::TempDir;

		use super::*;

		const PLAIN: Style = Style { colour: false };

		fn entry(ts: &str, message: &str) -> String {
			format!(
				r#"{{"timestamp":"{ts}","level":"INFO","fields":{{"message":"{message}"}},"target":"bestool"}}"#
			)
		}

		fn write(path: &Path, lines: &[String]) {
			let mut file = std::fs::OpenOptions::new()
				.create(true)
				.append(true)
				.open(path)
				.unwrap();
			for line in lines {
				writeln!(file, "{line}").unwrap();
			}
		}

		fn texts(entries: &[Entry]) -> Vec<&str> {
			entries.iter().map(|entry| entry.text.as_str()).collect()
		}

		#[test]
		fn recent_merges_files_by_timestamp_and_keeps_the_last_n() {
			let dir = TempDir::new().unwrap();
			write(
				&dir.path()
					.join("bestool.2026-09-29T01-00-00Z.log.2026-09-29"),
				&[
					entry("2026-09-29T01:00:00.000001Z", "a"),
					entry("2026-09-29T01:00:02.000000Z", "c"),
				],
			);
			write(
				&dir.path()
					.join("bestool.2026-09-29T01-00-01Z.log.2026-09-29"),
				&[
					entry("2026-09-29T01:00:01.000000Z", "b"),
					entry("2026-09-29T01:00:03.000000Z", "d"),
				],
			);

			let mut logs = LogDir::new(dir.path().to_owned(), PLAIN);
			let recent = logs.recent(3).unwrap();
			assert_eq!(
				texts(&recent),
				[
					"2026-09-29T01:00:01.000000Z  INFO bestool: b",
					"2026-09-29T01:00:02.000000Z  INFO bestool: c",
					"2026-09-29T01:00:03.000000Z  INFO bestool: d",
				]
			);
		}

		#[test]
		fn recent_leaves_a_partial_line_for_the_follow() {
			let dir = TempDir::new().unwrap();
			let path = dir.path().join("bestool.log");
			write(&path, &[entry("2026-09-29T01:00:00.000000Z", "done")]);
			let partial = entry("2026-09-29T01:00:01.000000Z", "later");
			let (head, tail) = partial.split_at(20);
			std::fs::OpenOptions::new()
				.append(true)
				.open(&path)
				.unwrap()
				.write_all(head.as_bytes())
				.unwrap();

			let mut logs = LogDir::new(dir.path().to_owned(), PLAIN);
			assert_eq!(
				texts(&logs.recent(50).unwrap()),
				["2026-09-29T01:00:00.000000Z  INFO bestool: done"]
			);

			writeln!(
				std::fs::OpenOptions::new()
					.append(true)
					.open(&path)
					.unwrap(),
				"{tail}"
			)
			.unwrap();
			assert_eq!(
				texts(&logs.poll().unwrap()),
				["2026-09-29T01:00:01.000000Z  INFO bestool: later"]
			);
		}

		#[test]
		fn poll_follows_appends_and_picks_up_new_files_from_their_start() {
			let dir = TempDir::new().unwrap();
			let first = dir
				.path()
				.join("bestool.2026-09-29T01-00-00Z.log.2026-09-29");
			write(&first, &[entry("2026-09-29T01:00:00.000000Z", "old")]);

			let mut logs = LogDir::new(dir.path().to_owned(), PLAIN);
			logs.recent(50).unwrap();
			assert!(logs.poll().unwrap().is_empty());

			write(&first, &[entry("2026-09-29T01:00:05.000000Z", "appended")]);
			// A restarted daemon's new series, already holding entries.
			write(
				&dir.path()
					.join("bestool.2026-09-29T01-00-06Z.log.2026-09-29"),
				&[
					entry("2026-09-29T01:00:06.000000Z", "restarted"),
					entry("2026-09-29T01:00:07.000000Z", "running"),
				],
			);
			assert_eq!(
				texts(&logs.poll().unwrap()),
				[
					"2026-09-29T01:00:05.000000Z  INFO bestool: appended",
					"2026-09-29T01:00:06.000000Z  INFO bestool: restarted",
					"2026-09-29T01:00:07.000000Z  INFO bestool: running",
				]
			);
		}

		#[test]
		fn poll_forgets_deleted_files() {
			let dir = TempDir::new().unwrap();
			let path = dir.path().join("bestool.log.2026-08-01");
			write(&path, &[entry("2026-08-01T00:00:00.000000Z", "old")]);

			let mut logs = LogDir::new(dir.path().to_owned(), PLAIN);
			logs.recent(50).unwrap();
			std::fs::remove_file(&path).unwrap();
			assert!(logs.poll().unwrap().is_empty());
			assert!(logs.cursors.is_empty());
		}

		#[test]
		fn a_line_that_is_not_an_entry_stays_after_the_one_it_followed() {
			let dir = TempDir::new().unwrap();
			write(
				&dir.path().join("a.log"),
				&[
					entry("2026-09-29T01:00:00.000000Z", "first"),
					"not json".to_owned(),
				],
			);
			write(
				&dir.path().join("b.log"),
				&[entry("2026-09-29T01:00:01.000000Z", "second")],
			);

			let mut logs = LogDir::new(dir.path().to_owned(), PLAIN);
			assert_eq!(
				texts(&logs.recent(50).unwrap()),
				[
					"2026-09-29T01:00:00.000000Z  INFO bestool: first",
					"not json",
					"2026-09-29T01:00:01.000000Z  INFO bestool: second",
				]
			);
		}

		#[test]
		fn a_missing_directory_names_where_it_looked() {
			let dir = TempDir::new().unwrap();
			let missing = dir.path().join("logs");
			let err = LogDir::new(missing.clone(), PLAIN).recent(50).unwrap_err();
			let message = err.to_string();
			assert!(
				message.contains(&missing.display().to_string()),
				"{message}"
			);
		}

		#[test]
		fn an_empty_directory_names_where_it_looked() {
			let dir = TempDir::new().unwrap();
			let err = LogDir::new(dir.path().to_owned(), PLAIN)
				.recent(50)
				.unwrap_err();
			let message = err.to_string();
			assert!(
				message.contains(&dir.path().display().to_string()),
				"{message}"
			);
		}

		#[test]
		fn permission_denied_directs_to_an_elevated_shell() {
			let err = io::Error::from(io::ErrorKind::PermissionDenied);
			let message = unreadable(Path::new("C:/logs/bestool.log"), &err).to_string();
			assert!(message.contains("elevated shell"), "{message}");
		}

		#[test]
		fn read_tail_spans_chunks() {
			let dir = TempDir::new().unwrap();
			let path = dir.path().join("big.log");
			let lines: Vec<String> = (0..2000).map(|i| format!("line {i:05}")).collect();
			write(&path, &lines);

			let (tail, pos) = read_tail(&path, 3).unwrap();
			assert_eq!(tail, ["line 01997", "line 01998", "line 01999"]);
			assert_eq!(pos, std::fs::metadata(&path).unwrap().len());
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const PLAIN: Style = Style { colour: false };

	#[test]
	fn renders_an_entry_as_the_terminal_does() {
		let line = r#"{"timestamp":"2026-09-30T02:36:29.855194Z","level":"WARN","fields":{"message":"self-update failed","latest":"2.2.5","attempt":2},"target":"bestool::actions::self_update::task"}"#;
		let (at, text) = render_json_entry(line, PLAIN).unwrap();
		assert_eq!(
			at,
			"2026-09-30T02:36:29.855194Z".parse::<Timestamp>().unwrap()
		);
		assert_eq!(
			text,
			"2026-09-30T02:36:29.855194Z  WARN bestool::actions::self_update::task: self-update failed latest=2.2.5 attempt=2"
		);
	}

	#[test]
	fn renders_spans_outermost_first() {
		let line = r#"{"timestamp":"2026-09-30T02:36:29.855194Z","level":"INFO","fields":{"message":"tick"},"target":"bestool::alertd","span":{"name":"task","task":"doctor"},"spans":[{"name":"daemon"},{"name":"task","task":"doctor"}]}"#;
		let (_, text) = render_json_entry(line, PLAIN).unwrap();
		assert_eq!(
			text,
			"2026-09-30T02:36:29.855194Z  INFO daemon:task{task=doctor}: bestool::alertd: tick"
		);
	}

	#[test]
	fn renders_log_bridged_events_under_their_own_target() {
		let line = r#"{"timestamp":"2026-09-30T02:36:29.855194Z","level":"DEBUG","fields":{"message":"connecting","log.target":"hyper::client","log.module_path":"hyper::client","log.file":"src/client.rs","log.line":12},"target":"log"}"#;
		let (_, text) = render_json_entry(line, PLAIN).unwrap();
		assert_eq!(
			text,
			"2026-09-30T02:36:29.855194Z DEBUG hyper::client: connecting"
		);
	}

	#[test]
	fn a_line_that_is_not_an_entry_does_not_render() {
		assert!(render_json_entry("", PLAIN).is_none());
		assert!(render_json_entry(r#"{"timestamp":"2026-09-30T02:3"#, PLAIN).is_none());
		assert!(render_json_entry(r#"{"level":"INFO"}"#, PLAIN).is_none());
		assert!(render_json_entry(r#"["not", "an", "object"]"#, PLAIN).is_none());
	}

	#[test]
	fn renders_a_journal_record_with_its_timestamp() {
		let line = r#"{"__REALTIME_TIMESTAMP":"1790735789855194","MESSAGE":" INFO bestool::alertd: tick","SYSLOG_IDENTIFIER":"bestool-alertd"}"#;
		let at = Timestamp::from_microsecond(1_790_735_789_855_194).unwrap();
		assert_eq!(
			render_journal_record(line, PLAIN).unwrap(),
			format!("{at:.6}  INFO bestool::alertd: tick")
		);
		assert!(format!("{at:.6}").ends_with(".855194Z"));
	}

	#[test]
	fn renders_a_journal_record_whose_message_is_bytes() {
		let line =
			r#"{"__REALTIME_TIMESTAMP":"1790735789000000","MESSAGE":[32,73,78,70,79,32,104,105]}"#;
		let text = render_journal_record(line, PLAIN).unwrap();
		assert!(text.ends_with("  INFO hi"), "{text}");
	}

	#[test]
	fn a_journal_record_without_a_message_does_not_render() {
		assert!(render_journal_record(r#"{"__REALTIME_TIMESTAMP":"1"}"#, PLAIN).is_none());
	}

	#[test]
	fn printing_stops_when_the_reader_goes_away() {
		struct Closed;
		impl Write for Closed {
			fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
				Err(std::io::ErrorKind::BrokenPipe.into())
			}
			fn flush(&mut self) -> std::io::Result<()> {
				Ok(())
			}
		}
		assert!(!print_lines(&mut Closed, ["a"]));
		let mut buf = Vec::new();
		assert!(print_lines(&mut buf, ["a", "b"]));
		assert_eq!(buf, b"a\nb\n");
	}
}
