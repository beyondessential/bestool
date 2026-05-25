//! Postgres backup recency via kopia.
//!
//! Asks kopia for the snapshot list scoped to the current host (`--no-all`
//! undoes the implicit `--all` that `--json` carries) and picks the newest
//! entry whose `source.path` looks like a PostgreSQL data directory. Kopia
//! repos are typically shared between many servers; letting kopia do the
//! host filtering means we get exactly *this* host's snapshots, computed
//! from kopia's own source identity rather than us trying to match
//! hostnames ourselves.
//!
//! On Linux the kopia repository config lives in the `kopia` system user's
//! home directory and isn't readable by other users (not even to check for
//! existence), so we don't pre-flight that; instead we either run kopia as
//! that user directly (when the doctor is invoked as `kopia`) or via
//! `runuser -u kopia --` (when invoked as root). Other contexts (e.g. the
//! alertd daemon running as `tamanu`) Skip with a reason — there's no safe
//! way to query an arbitrary user's kopia repo. If kopia is installed but
//! not connected to a repo, the snapshot-list invocation reports that and
//! we Skip with the error.
//!
//! On Windows the backup is set up via KopiaUI under a desktop user (typically
//! Administrator); we locate the bundled `kopia.exe` plus the user's
//! `%APPDATA%\kopia\repository.config` and run it as the current user.

use std::{
	path::{Path, PathBuf},
	process::Command,
};

use jiff::Timestamp;
use serde::Deserialize;

use super::CheckContext;
use crate::doctor::check::Check;

const CHECK_NAME: &str = "kopia_backup";
const LINUX_KOPIA_USER: &str = "kopia";
const FAIL_AGE_SECS: i64 = 24 * 60 * 60;

pub async fn run(_ctx: CheckContext) -> Check {
	if cfg!(target_os = "linux") {
		return run_linux();
	}
	if cfg!(target_os = "windows") {
		return run_windows();
	}
	Check::skip(
		CHECK_NAME,
		"not supported on this platform",
		"no kopia integration available outside Linux and Windows",
	)
}

fn run_linux() -> Check {
	// The kopia repository config at /var/lib/kopia/.config/kopia/repository.config
	// is only readable by the kopia user, so we can't pre-check for "kopia
	// configured"; we discover that by trying. If kopia isn't connected to a
	// repo, `kopia snapshot list` errors out and we Skip with the message.
	let kopia_binary = match find_unix_kopia_binary() {
		Some(b) => b,
		None => {
			return Check::skip(
				CHECK_NAME,
				"kopia binary not found",
				"could not find `kopia` in PATH",
			);
		}
	};

	let mut cmd = match build_linux_command(&kopia_binary) {
		Ok(c) => c,
		Err(skip) => return skip,
	};

	let output = match cmd
		.args(["snapshot", "list", "--json", "--no-all"])
		.env("KOPIA_CHECK_FOR_UPDATES", "false")
		.output()
	{
		Ok(o) if o.status.success() => o,
		Ok(o) => {
			let stderr = String::from_utf8_lossy(&o.stderr);
			return Check::skip(
				CHECK_NAME,
				"kopia snapshot list failed",
				format!("kopia exited {}: {}", o.status, stderr.trim()),
			);
		}
		Err(err) => {
			return Check::skip(
				CHECK_NAME,
				"kopia snapshot list failed",
				format!("could not invoke kopia: {err}"),
			);
		}
	};

	let snapshots: Vec<Snapshot> = match serde_json::from_slice(&output.stdout) {
		Ok(s) => s,
		Err(err) => {
			return Check::skip(
				CHECK_NAME,
				"kopia output unparseable",
				format!("could not decode kopia snapshot list JSON: {err}"),
			);
		}
	};

	evaluate(&snapshots, Timestamp::now())
		.with_detail("platform", "linux")
		.with_detail("kopia_binary", kopia_binary.display().to_string())
}

/// Build the kopia command, elevating to the kopia user if needed.
///
/// `Err(check)` carries a Skip result if we can't safely elevate (e.g. the
/// doctor is running as `tamanu` and can't `runuser`).
fn build_linux_command(kopia_binary: &Path) -> Result<Command, Check> {
	let current = current_unix_username();
	match current.as_deref() {
		Some(u) if u == LINUX_KOPIA_USER => Ok(Command::new(kopia_binary)),
		Some("root") => {
			// `runuser -u kopia --` switches to the kopia user (only works when
			// we're already root, since runuser usually isn't setuid). Falls
			// back to the kopia binary as the executed program.
			let mut c = Command::new("runuser");
			c.arg("-u")
				.arg(LINUX_KOPIA_USER)
				.arg("--")
				.arg(kopia_binary);
			Ok(c)
		}
		Some(other) => Err(Check::skip(
			CHECK_NAME,
			"need elevation to query kopia",
			format!(
				"running as `{other}`, but the kopia config is owned by `{LINUX_KOPIA_USER}`; re-run as root or as the kopia user",
			),
		)),
		None => Err(Check::skip(
			CHECK_NAME,
			"current user unknown",
			"could not determine current Unix username via `id -un`",
		)),
	}
}

/// Returns the current process's username via `id -un`. Returns `None` on
/// non-Unix platforms or if `id` is unavailable.
#[cfg(unix)]
fn current_unix_username() -> Option<String> {
	let output = Command::new("id").arg("-un").output().ok()?;
	if !output.status.success() {
		return None;
	}
	let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
	if name.is_empty() { None } else { Some(name) }
}

#[cfg(not(unix))]
fn current_unix_username() -> Option<String> {
	None
}

/// Locate the kopia binary on Linux. Prefers PATH (kopia is installed at
/// `/usr/bin/kopia` on our servers via the kopia apt repo).
fn find_unix_kopia_binary() -> Option<PathBuf> {
	let path = std::env::var_os("PATH")?;
	for dir in std::env::split_paths(&path) {
		let candidate = dir.join("kopia");
		if candidate.is_file() {
			return Some(candidate);
		}
	}
	None
}

fn run_windows() -> Check {
	let Some(KopiaWindows { binary, config }) = locate_windows_kopia() else {
		return Check::skip(
			CHECK_NAME,
			"kopia not configured",
			"no KopiaUI install or repository config found for the current user",
		);
	};

	let output = match Command::new(&binary)
		.args(["snapshot", "list", "--json", "--no-all"])
		.env("KOPIA_CONFIG_PATH", &config)
		.env("KOPIA_CHECK_FOR_UPDATES", "false")
		.output()
	{
		Ok(o) if o.status.success() => o,
		Ok(o) => {
			let stderr = String::from_utf8_lossy(&o.stderr);
			return Check::skip(
				CHECK_NAME,
				"kopia snapshot list failed",
				format!("kopia exited {}: {}", o.status, stderr.trim()),
			);
		}
		Err(err) => {
			return Check::skip(
				CHECK_NAME,
				"kopia snapshot list failed",
				format!("could not invoke {}: {err}", binary.display()),
			);
		}
	};

	let snapshots: Vec<Snapshot> = match serde_json::from_slice(&output.stdout) {
		Ok(s) => s,
		Err(err) => {
			return Check::skip(
				CHECK_NAME,
				"kopia output unparseable",
				format!("could not decode kopia snapshot list JSON: {err}"),
			);
		}
	};

	evaluate(&snapshots, Timestamp::now())
		.with_detail("platform", "windows")
		.with_detail("kopia_binary", binary.display().to_string())
		.with_detail("kopia_config", config.display().to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
	source: SnapshotSource,
	#[serde(default)]
	end_time: Option<Timestamp>,
	#[serde(default)]
	start_time: Option<Timestamp>,
}

#[derive(Debug, Deserialize)]
struct SnapshotSource {
	#[serde(default)]
	path: String,
}

impl Snapshot {
	fn taken_at(&self) -> Option<Timestamp> {
		self.end_time.or(self.start_time)
	}
}

fn evaluate(snapshots: &[Snapshot], now: Timestamp) -> Check {
	// Host filtering is delegated to kopia via `--no-all`; everything in
	// `snapshots` is already scoped to this host's source identity. We only
	// need to pick the postgres-shaped one.
	let candidates: Vec<&Snapshot> = snapshots
		.iter()
		.filter(|s| is_postgres_path(&s.source.path))
		.collect();

	if candidates.is_empty() {
		return Check::fail(
			CHECK_NAME,
			"no postgres snapshots found",
			"kopia is configured but no snapshot for this host has a source path matching PostgreSQL",
		);
	}

	let Some(latest) = candidates
		.iter()
		.filter_map(|s| s.taken_at().map(|t| (s, t)))
		.max_by_key(|(_, t)| *t)
	else {
		return Check::fail(
			CHECK_NAME,
			"no completed postgres snapshots",
			"found postgres snapshots but none have a start or end timestamp",
		);
	};

	let (snapshot, taken_at) = latest;
	let age_secs = (now - taken_at).get_seconds();
	let summary = format!("last backup {} ago", humanise_age(age_secs));

	let check = if age_secs >= FAIL_AGE_SECS {
		Check::fail(
			CHECK_NAME,
			summary.clone(),
			format!("no backup in {}", humanise_age(FAIL_AGE_SECS)),
		)
	} else {
		Check::pass(CHECK_NAME, summary)
	};

	check
		.with_detail("source_path", snapshot.source.path.clone())
		.with_detail("last_snapshot", taken_at.to_string())
		.with_detail("age_secs", age_secs)
}

/// Heuristic match for "this snapshot is of PostgreSQL data". Standard
/// installs put data under `/var/lib/postgresql/...` on Linux and
/// `C:\Program Files\PostgreSQL` on Windows; we tolerate variations
/// (different drive, custom install dir) by just looking for the directory
/// name anywhere in the path.
fn is_postgres_path(path: &str) -> bool {
	path.to_lowercase().contains("postgresql")
}

struct KopiaWindows {
	binary: PathBuf,
	config: PathBuf,
}

fn locate_windows_kopia() -> Option<KopiaWindows> {
	let binary = locate_windows_kopia_binary()?;
	let config = locate_windows_kopia_config()?;
	Some(KopiaWindows { binary, config })
}

fn locate_windows_kopia_binary() -> Option<PathBuf> {
	let mut candidates: Vec<PathBuf> = Vec::new();
	if let Ok(local) = std::env::var("LOCALAPPDATA") {
		candidates.push(
			Path::new(&local)
				.join("Programs")
				.join("KopiaUI")
				.join("resources")
				.join("server")
				.join("kopia.exe"),
		);
	}
	if let Ok(pf) = std::env::var("ProgramFiles") {
		candidates.push(
			Path::new(&pf)
				.join("KopiaUI")
				.join("resources")
				.join("server")
				.join("kopia.exe"),
		);
	}
	if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
		candidates.push(
			Path::new(&pf86)
				.join("KopiaUI")
				.join("resources")
				.join("server")
				.join("kopia.exe"),
		);
	}
	candidates.into_iter().find(|p| p.exists())
}

fn locate_windows_kopia_config() -> Option<PathBuf> {
	let appdata = std::env::var("APPDATA").ok()?;
	let config = Path::new(&appdata).join("kopia").join("repository.config");
	config.exists().then_some(config)
}

fn humanise_age(secs: i64) -> String {
	let secs = secs.max(0) as u64;
	if secs < 60 {
		format!("{secs}s")
	} else if secs < 3600 {
		format!("{}m", secs / 60)
	} else if secs < 86400 {
		format!("{}h", secs / 3600)
	} else {
		format!("{}d", secs / 86400)
	}
}

#[cfg(test)]
mod tests {
	use jiff::ToSpan;

	use super::*;
	use crate::doctor::check::CheckStatus;

	#[test]
	fn is_postgres_path_matches_program_files() {
		assert!(is_postgres_path(r"C:\Program Files\PostgreSQL\15\data"));
		assert!(is_postgres_path(r"D:\PostgreSQL"));
		assert!(is_postgres_path("/var/lib/postgresql/16/main"));
	}

	#[test]
	fn is_postgres_path_rejects_unrelated() {
		assert!(!is_postgres_path(r"C:\Users\admin\Documents"));
		assert!(!is_postgres_path(r"C:\Program Files\KopiaUI"));
	}

	fn snapshot(path: &str, end: Option<Timestamp>, start: Option<Timestamp>) -> Snapshot {
		Snapshot {
			source: SnapshotSource { path: path.into() },
			end_time: end,
			start_time: start,
		}
	}

	#[test]
	fn fail_when_no_postgres_snapshots() {
		let snapshots = vec![snapshot(
			r"C:\Users\admin\Documents",
			Some(Timestamp::now()),
			None,
		)];
		let check = evaluate(&snapshots, Timestamp::now());
		assert!(matches!(check.status, CheckStatus::Fail(_)), "{check:?}");
	}

	#[test]
	fn pass_when_recent_postgres_snapshot() {
		let now = Timestamp::from_second(20_000_000).unwrap();
		let snapshots = vec![snapshot(
			"/var/lib/postgresql/16/main",
			Some(now - 2.hours()),
			Some(now - 3.hours()),
		)];
		let check = evaluate(&snapshots, now);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn fail_when_postgres_snapshot_older_than_24h() {
		let now = Timestamp::from_second(20_000_000).unwrap();
		let snapshots = vec![snapshot(
			"/var/lib/postgresql/16/main",
			Some(now - 30.hours()),
			None,
		)];
		let check = evaluate(&snapshots, now);
		assert!(matches!(check.status, CheckStatus::Fail(_)), "{check:?}");
	}

	#[test]
	fn picks_newest_among_postgres_sources() {
		let now = Timestamp::from_second(20_000_000).unwrap();
		let snapshots = vec![
			snapshot(
				r"C:\Program Files\PostgreSQL\15",
				Some(now - 30.hours()),
				None,
			),
			snapshot(r"D:\Backups\PostgreSQL", Some(now - 1.hour()), None),
			snapshot(r"C:\Users\admin", Some(now - 10.minutes()), None),
		];
		let check = evaluate(&snapshots, now);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn falls_back_to_start_time_when_end_missing() {
		let now = Timestamp::from_second(20_000_000).unwrap();
		let snapshots = vec![snapshot(
			"/var/lib/postgresql/16/main",
			None,
			Some(now - 1.hour()),
		)];
		let check = evaluate(&snapshots, now);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn humanise_age_formats_units() {
		assert_eq!(humanise_age(30), "30s");
		assert_eq!(humanise_age(120), "2m");
		assert_eq!(humanise_age(3600), "1h");
		assert_eq!(humanise_age(86400 * 2), "2d");
		assert_eq!(humanise_age(-5), "0s");
	}
}
