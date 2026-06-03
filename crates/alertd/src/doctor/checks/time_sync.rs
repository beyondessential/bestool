use tokio::process::Command;

use super::CheckContext;
use crate::doctor::check::Check;

pub async fn run(_ctx: CheckContext) -> Check {
	if cfg!(target_os = "linux") {
		return run_linux().await;
	}
	if cfg!(target_os = "windows") {
		return run_windows().await;
	}
	Check::skip(
		"time_sync",
		"not supported on this platform",
		"no time-sync probe available outside Linux and Windows",
	)
}

async fn run_linux() -> Check {
	let output = match Command::new("timedatectl")
		.args(["show", "-p", "NTPSynchronized", "--value"])
		.output()
		.await
	{
		Ok(o) if o.status.success() => o,
		Ok(_) | Err(_) => {
			return Check::skip(
				"time_sync",
				"timedatectl unavailable",
				"could not run timedatectl",
			)
			.with_detail("synchronized", serde_json::Value::Null);
		}
	};
	let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
	let synced = stdout == "yes";

	let check = if synced {
		Check::pass("time_sync", "NTP synchronised")
	} else {
		Check::warning(
			"time_sync",
			"NTP not synchronised",
			"timedatectl reports no",
		)
	};
	check
		.with_detail("synchronized", synced)
		.with_detail("service", "timedatectl")
}

/// Probe Windows time service via `w32tm /query /status`.
///
/// We look for two signals:
///   * a `Source:` line that *isn't* "Local CMOS Clock" / "Free-running System
///     Clock" — anything else (an NTP peer, domain controller, etc.) means the
///     machine is genuinely getting time from somewhere upstream;
///   * a `Leap Indicator:` line that contains "no warning" — the standard
///     "all good" sentinel.
///
/// If neither line is present (older Windows builds, locked-down configs), we
/// skip rather than guessing.
async fn run_windows() -> Check {
	let output = match Command::new("w32tm")
		.args(["/query", "/status"])
		.output()
		.await
	{
		Ok(o) if o.status.success() => o,
		Ok(_) | Err(_) => {
			return Check::skip(
				"time_sync",
				"w32tm unavailable",
				"could not run w32tm /query /status (W32Time service not running?)",
			)
			.with_detail("synchronized", serde_json::Value::Null);
		}
	};
	let stdout = String::from_utf8_lossy(&output.stdout).to_string();
	let parsed = parse_w32tm_status(&stdout);

	let check = match parsed.synchronized {
		Some(true) => Check::pass(
			"time_sync",
			format!(
				"NTP synchronised{}",
				parsed
					.source
					.as_deref()
					.map(|s| format!(" via {s}"))
					.unwrap_or_default()
			),
		),
		Some(false) => Check::warning(
			"time_sync",
			"NTP not synchronised",
			parsed
				.source
				.as_deref()
				.map(|s| format!("source is {s}"))
				.unwrap_or_else(|| "w32tm reports unsynchronised".to_string()),
		),
		None => Check::skip(
			"time_sync",
			"w32tm output unparseable",
			"could not determine synchronisation state from w32tm output",
		),
	};
	let mut check = check.with_detail("service", "w32tm");
	if let Some(s) = parsed.source {
		check = check.with_detail("source", s);
	}
	if let Some(b) = parsed.synchronized {
		check = check.with_detail("synchronized", b);
	}
	check
}

#[derive(Debug, Default, PartialEq, Eq)]
struct W32TmStatus {
	source: Option<String>,
	synchronized: Option<bool>,
}

fn parse_w32tm_status(text: &str) -> W32TmStatus {
	let mut out = W32TmStatus::default();
	for line in text.lines() {
		let line = line.trim();
		if let Some(rest) = line.strip_prefix("Source:") {
			let s = rest.trim().to_string();
			if !s.is_empty() {
				out.source = Some(s);
			}
		} else if let Some(rest) = line.strip_prefix("Leap Indicator:") {
			// Sample: "Leap Indicator: 0(no warning)". "no warning" is the
			// good signal; anything else (or absent) means not synced.
			out.synchronized = Some(rest.to_lowercase().contains("no warning"));
		}
	}
	// If we never saw the Leap Indicator line but the source is the local
	// clock, treat that as "not synced from upstream".
	if out.synchronized.is_none()
		&& let Some(src) = out.source.as_deref()
		&& (src.eq_ignore_ascii_case("Local CMOS Clock")
			|| src.eq_ignore_ascii_case("Free-running System Clock"))
	{
		out.synchronized = Some(false);
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_w32tm_synchronised_with_peer() {
		let text = "Leap Indicator: 0(no warning)\n\
		            Stratum: 3 (secondary reference)\n\
		            Source: time.windows.com,0x1\n";
		assert_eq!(
			parse_w32tm_status(text),
			W32TmStatus {
				source: Some("time.windows.com,0x1".into()),
				synchronized: Some(true),
			}
		);
	}

	#[test]
	fn parses_w32tm_unsynchronised_local_clock() {
		let text = "Stratum: 0\nSource: Local CMOS Clock\n";
		assert_eq!(
			parse_w32tm_status(text),
			W32TmStatus {
				source: Some("Local CMOS Clock".into()),
				synchronized: Some(false),
			}
		);
	}

	#[test]
	fn parses_w32tm_leap_other_than_no_warning_is_unsync() {
		let text = "Leap Indicator: 3(not synchronised)\nSource: foo\n";
		assert_eq!(
			parse_w32tm_status(text).synchronized,
			Some(false),
			"any leap indicator that isn't \"no warning\" means out of sync"
		);
	}

	#[test]
	fn parses_w32tm_unparseable_yields_none() {
		assert_eq!(parse_w32tm_status("garbage\n\n").synchronized, None);
	}
}
