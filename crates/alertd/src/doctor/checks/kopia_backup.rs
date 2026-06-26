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
//! existence), so we don't pre-flight that; we ask
//! `bestool_kopia::build_kopia_command` to elevate via `sudo -u kopia --`
//! when we're a different user and the system kopia install is present, or
//! run directly when we *are* the kopia user. If sudo isn't allowed (no
//! NOPASSWD rule, no TTY in the alertd context), the resulting kopia
//! invocation fails and we Skip with the error.
//!
//! On Windows the backup is set up via KopiaUI under a desktop user
//! (typically Administrator); we locate the bundled `kopia.exe` plus the
//! user's `%APPDATA%\kopia\repository.config` and run it as the current
//! user.

use bestool_kopia::{Elevation, Snapshot};
use jiff::Timestamp;

use super::SweepContext;
use crate::doctor::check::Check;

const CHECK_NAME: &str = "kopia_backup";
const WARN_AGE_SECS: i64 = 12 * 60 * 60;
const FAIL_AGE_SECS: i64 = 24 * 60 * 60;

pub async fn run(_ctx: SweepContext) -> Check {
	if cfg!(target_os = "linux") {
		return run_linux().await;
	}
	if cfg!(target_os = "windows") {
		return run_windows().await;
	}
	Check::skip(
		CHECK_NAME,
		"not supported on this platform",
		"no kopia integration available outside Linux and Windows",
	)
}

async fn run_linux() -> Check {
	let Some(kopia_binary) = bestool_kopia::find_kopia_binary(None) else {
		return Check::skip(
			CHECK_NAME,
			"kopia binary not found",
			"could not find `kopia` in PATH",
		);
	};

	let elevation = bestool_kopia::linux_elevation();
	if let Elevation::Skip(reason) = &elevation {
		return Check::skip(CHECK_NAME, "need elevation to query kopia", reason.clone());
	}

	let mut cmd = match bestool_kopia::build_kopia_command(&kopia_binary) {
		Ok(c) => c,
		Err(reason) => {
			return Check::skip(CHECK_NAME, "need elevation to query kopia", reason);
		}
	};

	cmd.args(["snapshot", "list", "--json", "--no-all"])
		.env("KOPIA_CHECK_FOR_UPDATES", "false");
	let output = match tokio::process::Command::from(cmd).output().await {
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
			// kopia succeeded but handed us output we can't decode, so the check
			// couldn't run — broken, not skip.
			return Check::broken(
				CHECK_NAME,
				"kopia output unparseable",
				format!("could not decode kopia snapshot list JSON: {err}"),
			);
		}
	};

	evaluate(&snapshots, Timestamp::now())
		.with_detail("platform", "linux")
		.with_detail("kopia_binary", kopia_binary.display().to_string())
		.with_detail("elevation", elevation_label(&elevation))
}

fn elevation_label(e: &Elevation) -> &'static str {
	match e {
		Elevation::Direct => "direct",
		Elevation::SetPriv => "setpriv",
		Elevation::Sudo => "sudo",
		Elevation::Skip(_) => "skip",
	}
}

async fn run_windows() -> Check {
	let Some(binary) = bestool_kopia::find_kopia_binary(None) else {
		return Check::skip(
			CHECK_NAME,
			"kopia not configured",
			"no KopiaUI install found",
		);
	};
	let Some(config) = bestool_kopia::find_windows_kopia_config() else {
		return Check::skip(
			CHECK_NAME,
			"kopia not configured",
			"no kopia repository config found for the current user",
		);
	};

	let output = match tokio::process::Command::new(&binary)
		.args(["snapshot", "list", "--json", "--no-all"])
		.env("KOPIA_CONFIG_PATH", &config)
		.env("KOPIA_CHECK_FOR_UPDATES", "false")
		.output()
		.await
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
			// kopia succeeded but handed us output we can't decode, so the check
			// couldn't run — broken, not skip.
			return Check::broken(
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

fn evaluate(snapshots: &[Snapshot], now: Timestamp) -> Check {
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
	} else if age_secs >= WARN_AGE_SECS {
		Check::warning(
			CHECK_NAME,
			summary.clone(),
			format!("no backup in {}", humanise_age(WARN_AGE_SECS)),
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
/// `C:\Program Files\PostgreSQL` on Windows; tolerate variations (different
/// drive, custom install dir) by looking for the directory name anywhere in
/// the path.
fn is_postgres_path(path: &str) -> bool {
	path.to_lowercase().contains("postgresql")
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
	use std::collections::BTreeMap;

	use bestool_kopia::SnapshotSource;
	use jiff::ToSpan;

	use super::*;
	use crate::doctor::check::CheckStatus;

	fn snapshot(path: &str, end: Option<Timestamp>, start: Option<Timestamp>) -> Snapshot {
		Snapshot {
			id: "kabc".into(),
			source: SnapshotSource {
				host: "host-1".into(),
				user_name: "kopia".into(),
				path: path.into(),
			},
			description: String::new(),
			end_time: end,
			start_time: start,
			tags: BTreeMap::new(),
			root_entry: None,
		}
	}

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
	fn warn_when_postgres_snapshot_between_12h_and_24h() {
		let now = Timestamp::from_second(20_000_000).unwrap();
		let snapshots = vec![snapshot(
			"/var/lib/postgresql/16/main",
			Some(now - 18.hours()),
			None,
		)];
		let check = evaluate(&snapshots, now);
		assert!(matches!(check.status, CheckStatus::Warning(_)), "{check:?}");
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
