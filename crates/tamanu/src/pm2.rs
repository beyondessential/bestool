//! pm2 discovery. pm2 only runs on Windows in our deployments, but this
//! module compiles cross-platform — on non-Windows targets it's effectively
//! dead code, which keeps the doctor check's supervisor switch simple.
//!
//! Two paths into a process list:
//!
//! 1. Find and invoke the `pm2` CLI (`pm2 jlist`). Command::new on Windows
//!    doesn't apply PATHEXT, so we try `pm2.cmd` and `pm2.bat` explicitly,
//!    then probe common npm-global install locations for hosts where the
//!    npm bin directory isn't on PATH.
//! 2. If the CLI isn't reachable, read `PM2_HOME/dump.pm2` directly and
//!    cross-reference each entry's pid file under `PM2_HOME/pids/` against
//!    the OS process list. This bypasses pm2 entirely.
//!
//! Fallback (2) is degraded vs (1): it only sees processes that were in the
//! list at the last `pm2 save`. Tamanu deployments save their pm2 config at
//! install time and rarely mutate it afterwards, so in practice the dump
//! reflects the current intent.

use std::{
	path::{Path, PathBuf},
	process::Command,
};

use serde::Deserialize;
use serde_json::Value;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tracing::debug;

/// Environment variable override for the pm2 CLI to invoke. If set, this is
/// tried first ahead of any auto-discovery.
const PM2_COMMAND_ENV: &str = "BESTOOL_PM2_COMMAND";

#[derive(Clone, Debug)]
pub struct PmProc {
	pub name: String,
	pub pm_id: Option<i64>,
	pub running: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Source {
	Cli,
	Dump,
}

impl Source {
	pub fn as_str(self) -> &'static str {
		match self {
			Source::Cli => "cli",
			Source::Dump => "dump",
		}
	}
}

/// Discover pm2-managed processes. Tries the CLI first; if that fails for any
/// reason, falls back to reading `PM2_HOME/dump.pm2` plus pid files. The
/// returned `Source` records which path was taken — useful in diagnostics
/// since the dump-based fallback is stale-tolerant.
pub fn list() -> Result<(Vec<PmProc>, Source), String> {
	match list_via_cli() {
		Ok(procs) => Ok((procs, Source::Cli)),
		Err(cli_err) => {
			debug!(%cli_err, "pm2 CLI unreachable, falling back to dump.pm2");
			match list_via_dump() {
				Ok(procs) => Ok((procs, Source::Dump)),
				Err(dump_err) => Err(format!(
					"could not list pm2 processes: cli: {cli_err}; dump: {dump_err}"
				)),
			}
		}
	}
}

/// Locate a usable pm2 CLI executable. Order of probes:
///
/// 1. `BESTOOL_PM2_COMMAND` env var, if set.
/// 2. `pm2.cmd` / `pm2.bat` via PATH (Command::new doesn't expand PATHEXT).
/// 3. Common npm-global install locations.
pub fn find_command() -> Option<PathBuf> {
	if let Ok(p) = std::env::var(PM2_COMMAND_ENV) {
		let path = PathBuf::from(&p);
		if probe(&path) {
			return Some(path);
		}
	}

	for name in ["pm2.cmd", "pm2.bat"] {
		let path = PathBuf::from(name);
		if probe(&path) {
			return Some(path);
		}
	}

	for candidate in candidate_install_paths() {
		if probe(&candidate) {
			return Some(candidate);
		}
	}

	None
}

fn probe(path: &Path) -> bool {
	Command::new(path)
		.arg("--version")
		.output()
		.map(|o| o.status.success())
		.unwrap_or(false)
}

fn candidate_install_paths() -> Vec<PathBuf> {
	let mut out = Vec::new();
	let pm2 = |dir: PathBuf| dir.join("pm2.cmd");
	if let Ok(appdata) = std::env::var("APPDATA") {
		out.push(pm2(PathBuf::from(&appdata).join("npm")));
	}
	if let Ok(userprofile) = std::env::var("USERPROFILE") {
		out.push(pm2(PathBuf::from(&userprofile)
			.join("AppData")
			.join("Roaming")
			.join("npm")));
	}
	if let Ok(pf) = std::env::var("ProgramFiles") {
		out.push(pm2(PathBuf::from(&pf).join("nodejs")));
	}
	if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
		out.push(pm2(PathBuf::from(&pf86).join("nodejs")));
	}
	// Standalone install layout some Tamanu Windows hosts use:
	out.push(PathBuf::from(r"C:\pm2\pm2.cmd"));
	out
}

fn list_via_cli() -> Result<Vec<PmProc>, String> {
	let cmd = find_command().ok_or_else(|| "pm2 not found".to_string())?;
	let output = Command::new(&cmd)
		.arg("jlist")
		.output()
		.map_err(|e| format!("running {cmd:?}: {e}"))?;
	if !output.status.success() {
		return Err(format!(
			"{cmd:?} jlist failed: {}",
			String::from_utf8_lossy(&output.stderr).trim()
		));
	}
	let parsed: Value =
		serde_json::from_slice(&output.stdout).map_err(|e| format!("parse jlist: {e}"))?;
	let mut out = Vec::new();
	let Some(procs) = parsed.as_array() else {
		return Ok(out);
	};
	for p in procs {
		let Some(name) = p["name"].as_str() else {
			continue;
		};
		let state = p["pm2_env"]["status"].as_str().unwrap_or("unknown");
		let pm_id = p["pm_id"].as_i64();
		out.push(PmProc {
			name: name.to_string(),
			pm_id,
			running: state == "online",
		});
	}
	Ok(out)
}

/// Candidate locations for PM2_HOME, in priority order. The env var wins; then
/// the `C:\pm2` standalone layout some Tamanu Windows hosts use; then the
/// per-user default `$HOME/.pm2` (i.e. `%USERPROFILE%\.pm2` on Windows).
pub fn pm2_home_candidates() -> Vec<PathBuf> {
	let mut out = Vec::new();
	if let Ok(h) = std::env::var("PM2_HOME") {
		out.push(PathBuf::from(h));
	}
	out.push(PathBuf::from(r"C:\pm2"));
	if let Some(home) = dirs::home_dir() {
		out.push(home.join(".pm2"));
	}
	out
}

fn list_via_dump() -> Result<Vec<PmProc>, String> {
	let candidates = pm2_home_candidates();
	if candidates.is_empty() {
		return Err("no PM2_HOME candidate locations".to_string());
	}
	let mut errs = Vec::new();
	for home in &candidates {
		if !home.join("dump.pm2").is_file() {
			continue;
		}
		match list_via_dump_at(home) {
			Ok(procs) => return Ok(procs),
			Err(e) => errs.push(format!("{}: {e}", home.display())),
		}
	}
	if errs.is_empty() {
		Err(format!(
			"no dump.pm2 found in any of: {}",
			candidates
				.iter()
				.map(|p| p.display().to_string())
				.collect::<Vec<_>>()
				.join(", ")
		))
	} else {
		Err(errs.join("; "))
	}
}

fn list_via_dump_at(home: &Path) -> Result<Vec<PmProc>, String> {
	let entries = read_dump(home)?;
	if entries.is_empty() {
		return Ok(Vec::new());
	}

	let mut sys = System::new();
	sys.refresh_processes_specifics(ProcessesToUpdate::All, false, ProcessRefreshKind::nothing());

	let mut out = Vec::new();
	for entry in entries {
		let pid = read_pid_file(home, &entry.name, entry.pm_id);
		let running = pid
			.map(|p| sys.process(Pid::from_u32(p)).is_some())
			.unwrap_or(false);
		out.push(PmProc {
			name: entry.name,
			pm_id: entry.pm_id,
			running,
		});
	}
	Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
struct DumpEntry {
	name: String,
	#[serde(default)]
	pm_id: Option<i64>,
	#[serde(default)]
	pm_out_log_path: Option<String>,
	#[serde(default)]
	pm_err_log_path: Option<String>,
	#[serde(default)]
	pm2_env: Option<DumpEntryEnv>,
}

/// pm2 stores log paths under `pm2_env` in jlist output and sometimes also at
/// the top level in `dump.pm2` (varies by version), so we look in both.
#[derive(Debug, Clone, Deserialize)]
struct DumpEntryEnv {
	#[serde(default)]
	pm_out_log_path: Option<String>,
	#[serde(default)]
	pm_err_log_path: Option<String>,
}

impl DumpEntry {
	fn out_log(&self) -> Option<&str> {
		self.pm_out_log_path.as_deref().or_else(|| {
			self.pm2_env
				.as_ref()
				.and_then(|e| e.pm_out_log_path.as_deref())
		})
	}
	fn err_log(&self) -> Option<&str> {
		self.pm_err_log_path.as_deref().or_else(|| {
			self.pm2_env
				.as_ref()
				.and_then(|e| e.pm_err_log_path.as_deref())
		})
	}
}

fn read_dump(home: &Path) -> Result<Vec<DumpEntry>, String> {
	let path = home.join("dump.pm2");
	let bytes = std::fs::read(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
	serde_json::from_slice(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))
}

/// A pm2-managed process's log file (stdout or stderr).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogSource {
	pub name: String,
	pub pm_id: Option<i64>,
	pub stream: LogStream,
	pub path: PathBuf,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LogStream {
	Out,
	Err,
}

impl LogStream {
	pub fn as_str(self) -> &'static str {
		match self {
			LogStream::Out => "out",
			LogStream::Err => "err",
		}
	}
}

/// Resolve log files for pm2 processes whose name is in `names`. Walks the
/// PM2_HOME candidates, reads `dump.pm2`, and returns `out` + `err` entries
/// per matching process instance.
pub fn log_sources(names: &[&str]) -> Result<Vec<LogSource>, String> {
	let candidates = pm2_home_candidates();
	let mut errs = Vec::new();
	for home in &candidates {
		if !home.join("dump.pm2").is_file() {
			continue;
		}
		match read_dump(home) {
			Ok(entries) => return Ok(collect_log_sources(&entries, names)),
			Err(e) => errs.push(format!("{}: {e}", home.display())),
		}
	}
	if errs.is_empty() {
		Err(format!(
			"no dump.pm2 found in any of: {}",
			candidates
				.iter()
				.map(|p| p.display().to_string())
				.collect::<Vec<_>>()
				.join(", ")
		))
	} else {
		Err(errs.join("; "))
	}
}

fn collect_log_sources(entries: &[DumpEntry], names: &[&str]) -> Vec<LogSource> {
	let mut out = Vec::new();
	for entry in entries {
		if !names.contains(&entry.name.as_str()) {
			continue;
		}
		if let Some(p) = entry.out_log() {
			out.push(LogSource {
				name: entry.name.clone(),
				pm_id: entry.pm_id,
				stream: LogStream::Out,
				path: PathBuf::from(p),
			});
		}
		if let Some(p) = entry.err_log() {
			out.push(LogSource {
				name: entry.name.clone(),
				pm_id: entry.pm_id,
				stream: LogStream::Err,
				path: PathBuf::from(p),
			});
		}
	}
	out
}

/// pm2 writes pid files at `${PM2_HOME}/pids/${name}-${pm_id}.pid`. For a
/// singleton (no pm_id in dump), we try `${name}.pid` as a fallback.
fn read_pid_file(home: &Path, name: &str, pm_id: Option<i64>) -> Option<u32> {
	let candidates: Vec<PathBuf> = match pm_id {
		Some(id) => vec![home.join("pids").join(format!("{name}-{id}.pid"))],
		None => vec![
			home.join("pids").join(format!("{name}.pid")),
			home.join("pids").join(format!("{name}-0.pid")),
		],
	};
	for path in candidates {
		if let Ok(contents) = std::fs::read_to_string(&path) {
			if let Ok(pid) = contents.trim().parse::<u32>() {
				return Some(pid);
			}
		}
	}
	None
}

#[cfg(test)]
mod tests {
	use std::fs;

	use tempfile::tempdir;

	use super::*;

	fn write_dump(home: &Path, json: &str) {
		fs::write(home.join("dump.pm2"), json).unwrap();
	}

	fn write_pid(home: &Path, name: &str, pm_id: Option<i64>, pid: u32) {
		let pids = home.join("pids");
		fs::create_dir_all(&pids).unwrap();
		let file = match pm_id {
			Some(id) => pids.join(format!("{name}-{id}.pid")),
			None => pids.join(format!("{name}.pid")),
		};
		fs::write(file, pid.to_string()).unwrap();
	}

	#[test]
	fn read_dump_parses_minimal_entry() {
		let tmp = tempdir().unwrap();
		write_dump(
			tmp.path(),
			r#"[{"name":"tamanu-api","pm_id":0},{"name":"tamanu-tasks","pm_id":1}]"#,
		);
		let entries = read_dump(tmp.path()).unwrap();
		assert_eq!(entries.len(), 2);
		assert_eq!(entries[0].name, "tamanu-api");
		assert_eq!(entries[0].pm_id, Some(0));
		assert_eq!(entries[1].name, "tamanu-tasks");
		assert_eq!(entries[1].pm_id, Some(1));
	}

	#[test]
	fn read_dump_tolerates_extra_fields() {
		let tmp = tempdir().unwrap();
		write_dump(
			tmp.path(),
			r#"[{"name":"x","pm_id":0,"script":"/opt/x.js","pm2_env":{"status":"online"}}]"#,
		);
		let entries = read_dump(tmp.path()).unwrap();
		assert_eq!(entries.len(), 1);
	}

	#[test]
	fn read_dump_missing_file_errors() {
		let tmp = tempdir().unwrap();
		let err = read_dump(tmp.path()).unwrap_err();
		assert!(err.contains("dump.pm2"), "got: {err}");
	}

	#[test]
	fn read_pid_file_with_id() {
		let tmp = tempdir().unwrap();
		write_pid(tmp.path(), "tamanu-api", Some(2), 4242);
		assert_eq!(read_pid_file(tmp.path(), "tamanu-api", Some(2)), Some(4242));
	}

	#[test]
	fn read_pid_file_without_id_tries_zero() {
		let tmp = tempdir().unwrap();
		write_pid(tmp.path(), "tamanu-tasks", Some(0), 1234);
		assert_eq!(read_pid_file(tmp.path(), "tamanu-tasks", None), Some(1234));
	}

	#[test]
	fn read_pid_file_missing_returns_none() {
		let tmp = tempdir().unwrap();
		assert_eq!(read_pid_file(tmp.path(), "nope", Some(0)), None);
	}

	#[test]
	fn list_via_dump_marks_running_when_pid_alive() {
		// Our own pid is necessarily alive; use it as the "running" candidate.
		let our_pid = std::process::id();
		let tmp = tempdir().unwrap();
		write_dump(tmp.path(), r#"[{"name":"tamanu-api","pm_id":0}]"#);
		write_pid(tmp.path(), "tamanu-api", Some(0), our_pid);

		let procs = list_via_dump_at(tmp.path()).unwrap();
		assert_eq!(procs.len(), 1);
		assert_eq!(procs[0].name, "tamanu-api");
		assert!(procs[0].running, "process should be reported running");
	}

	#[test]
	fn list_via_dump_marks_not_running_for_dead_pid() {
		let tmp = tempdir().unwrap();
		write_dump(tmp.path(), r#"[{"name":"tamanu-api","pm_id":0}]"#);
		// `u32::MAX` is well beyond any real pid on Linux or Windows. Pid 0
		// is taken on Windows (System Idle Process), so we can't use that.
		write_pid(tmp.path(), "tamanu-api", Some(0), u32::MAX);

		let procs = list_via_dump_at(tmp.path()).unwrap();
		assert_eq!(procs.len(), 1);
		assert!(!procs[0].running);
	}

	#[test]
	fn list_via_dump_handles_missing_pid_file() {
		let tmp = tempdir().unwrap();
		write_dump(tmp.path(), r#"[{"name":"orphan","pm_id":3}]"#);
		// no pid file written
		let procs = list_via_dump_at(tmp.path()).unwrap();
		assert_eq!(procs.len(), 1);
		assert!(!procs[0].running);
	}

	#[test]
	fn pm2_home_candidates_includes_c_pm2() {
		let candidates = pm2_home_candidates();
		assert!(
			candidates.iter().any(|c| c == Path::new(r"C:\pm2")),
			"candidates were {candidates:?}"
		);
	}

	#[test]
	fn collect_log_sources_picks_up_pm2_env_paths() {
		let entries: Vec<DumpEntry> = serde_json::from_str(
			r#"[
				{"name":"tamanu-api","pm_id":1,"pm2_env":{
					"pm_out_log_path":"c:/pm2/logs/tamanu-api-out-1.log",
					"pm_err_log_path":"c:/pm2/logs/tamanu-api-error-1.log"
				}},
				{"name":"tamanu-api","pm_id":3,"pm2_env":{
					"pm_out_log_path":"c:/pm2/logs/tamanu-api-out-3.log",
					"pm_err_log_path":"c:/pm2/logs/tamanu-api-error-3.log"
				}},
				{"name":"tamanu-tasks","pm_id":2,"pm2_env":{
					"pm_out_log_path":"c:/pm2/logs/tamanu-tasks-out-2.log",
					"pm_err_log_path":"c:/pm2/logs/tamanu-tasks-error-2.log"
				}}
			]"#,
		)
		.unwrap();

		let sources = collect_log_sources(&entries, &["tamanu-api"]);
		// 2 instances × (out + err)
		assert_eq!(sources.len(), 4);
		let names_and_ids: Vec<(&str, Option<i64>, LogStream)> = sources
			.iter()
			.map(|s| (s.name.as_str(), s.pm_id, s.stream))
			.collect();
		assert!(names_and_ids.contains(&("tamanu-api", Some(1), LogStream::Out)));
		assert!(names_and_ids.contains(&("tamanu-api", Some(1), LogStream::Err)));
		assert!(names_and_ids.contains(&("tamanu-api", Some(3), LogStream::Out)));
		assert!(names_and_ids.contains(&("tamanu-api", Some(3), LogStream::Err)));
	}

	#[test]
	fn collect_log_sources_accepts_top_level_paths() {
		let entries: Vec<DumpEntry> = serde_json::from_str(
			r#"[
				{"name":"tamanu-tasks","pm_id":2,
				 "pm_out_log_path":"/x/out.log",
				 "pm_err_log_path":"/x/err.log"}
			]"#,
		)
		.unwrap();
		let sources = collect_log_sources(&entries, &["tamanu-tasks"]);
		assert_eq!(sources.len(), 2);
		assert_eq!(sources[0].path, PathBuf::from("/x/out.log"));
		assert_eq!(sources[1].path, PathBuf::from("/x/err.log"));
	}

	#[test]
	fn collect_log_sources_filters_by_name() {
		let entries: Vec<DumpEntry> = serde_json::from_str(
			r#"[
				{"name":"tamanu-api","pm_id":1,"pm2_env":{"pm_out_log_path":"/api.log"}},
				{"name":"tamanu-tasks","pm_id":2,"pm2_env":{"pm_out_log_path":"/tasks.log"}}
			]"#,
		)
		.unwrap();
		let sources = collect_log_sources(&entries, &["tamanu-tasks"]);
		assert_eq!(sources.len(), 1);
		assert_eq!(sources[0].name, "tamanu-tasks");
	}
}
