//! Caddy version healthcheck.
//!
//! Runs `caddy version` and compares against platform-specific thresholds.
//! `caddy version` prints something like `v2.10.1 h1:fonY...=` (Go module
//! build info appended after the version). The leading `v` and any
//! whitespace-separated suffix are stripped before parsing as semver.
//!
//! Thresholds:
//!   * always warn when <2.10.2
//!   * Linux fails when <2.10.0 or (>2.10.2 and <2.11.4)
//!   * Windows fails when <2.7.6
//!
//! The 2.11.4 upper bound is intentional — 2.11.4 is not yet released, the
//! check pre-emptively flags any 2.10.x / 2.11.x point releases that aren't
//! 2.10.0–2.10.2 as unsafe on Linux until 2.11.4 ships.

use node_semver::Version;
use tokio::process::Command;

use bestool_tamanu::caddy;

use super::SweepContext;
use crate::doctor::check::Check;

const CHECK_NAME: &str = "caddy_version";

pub async fn run(_ctx: SweepContext) -> Check {
	let platform = match Platform::current() {
		Some(p) => p,
		None => {
			return Check::skip(
				CHECK_NAME,
				"not supported on this platform",
				"caddy version thresholds only defined for Linux and Windows",
			);
		}
	};

	let output = match Command::new(caddy::program()).arg("version").output().await {
		Ok(o) if o.status.success() => o,
		Ok(o) => {
			// caddy ran but couldn't tell us its version, so the check couldn't
			// run — that's broken, not a skip (which would imply a precondition
			// like caddy not being installed).
			let stderr = String::from_utf8_lossy(&o.stderr);
			return Check::broken(
				CHECK_NAME,
				"caddy version failed",
				format!("caddy exited {}: {}", o.status, stderr.trim()),
			);
		}
		Err(err) => {
			return Check::skip(
				CHECK_NAME,
				"caddy not found",
				format!("could not invoke `caddy version`: {err}"),
			);
		}
	};

	let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
	let version = match parse_caddy_version(&stdout) {
		Some(v) => v,
		None => {
			// caddy answered but with output we can't read a version from, so
			// the check couldn't run: broken, not skip.
			return Check::broken(
				CHECK_NAME,
				"caddy version unparseable",
				format!("could not parse semver from `caddy version` output: {stdout:?}"),
			);
		}
	};

	build_check(&version, platform).with_detail("version", version.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
	Linux,
	Windows,
}

impl Platform {
	fn current() -> Option<Self> {
		if cfg!(target_os = "linux") {
			Some(Platform::Linux)
		} else if cfg!(target_os = "windows") {
			Some(Platform::Windows)
		} else {
			None
		}
	}
}

/// Extract the semver from the first whitespace-separated token of caddy's
/// version output. Strips the leading `v` if present.
fn parse_caddy_version(output: &str) -> Option<Version> {
	let token = output.split_whitespace().next()?;
	let stripped = token.strip_prefix('v').unwrap_or(token);
	Version::parse(stripped).ok()
}

fn build_check(version: &Version, platform: Platform) -> Check {
	let v_2_7_6 = Version::parse("2.7.6").unwrap();
	let v_2_10_0 = Version::parse("2.10.0").unwrap();
	let v_2_10_2 = Version::parse("2.10.2").unwrap();
	let v_2_11_4 = Version::parse("2.11.4").unwrap();

	let fail_reason: Option<String> = match platform {
		Platform::Linux => {
			if *version < v_2_10_0 {
				Some(format!("caddy {version} is older than 2.10 on Linux"))
			} else if *version > v_2_10_2 && *version < v_2_11_4 {
				Some(format!(
					"caddy {version} is in the unsafe 2.10.3–2.11.3 range on Linux",
				))
			} else {
				None
			}
		}
		Platform::Windows => {
			if *version < v_2_7_6 {
				Some(format!("caddy {version} is older than 2.7.6 on Windows"))
			} else {
				None
			}
		}
	};

	if let Some(reason) = fail_reason {
		return Check::fail(CHECK_NAME, format!("caddy {version}"), reason);
	}

	if *version < v_2_10_2 {
		return Check::warning(
			CHECK_NAME,
			format!("caddy {version}"),
			format!("caddy {version} is older than 2.10.2"),
		);
	}

	Check::pass(CHECK_NAME, format!("caddy {version}"))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn v(s: &str) -> Version {
		Version::parse(s).unwrap()
	}

	#[test]
	fn parses_caddy_version_with_hash() {
		assert_eq!(
			parse_caddy_version("v2.10.1 h1:fonYabcdef="),
			Some(v("2.10.1"))
		);
	}

	#[test]
	fn parses_caddy_version_bare() {
		assert_eq!(parse_caddy_version("v2.10.2"), Some(v("2.10.2")));
	}

	#[test]
	fn parses_caddy_version_without_v_prefix() {
		assert_eq!(parse_caddy_version("2.10.2"), Some(v("2.10.2")));
	}

	#[test]
	fn parses_caddy_prerelease() {
		assert_eq!(
			parse_caddy_version("v2.11.0-beta.1 h1:abc="),
			Some(v("2.11.0-beta.1"))
		);
	}

	#[test]
	fn rejects_devel_build() {
		assert!(parse_caddy_version("(devel)").is_none());
	}

	#[test]
	fn rejects_empty() {
		assert!(parse_caddy_version("").is_none());
	}

	fn status(check: &Check) -> &'static str {
		use crate::doctor::check::CheckStatus::*;
		match check.status {
			Pass => "pass",
			Skip(_) => "skip",
			Warning(_) => "warning",
			Fail(_) => "fail",
			Broken(_) => "broken",
		}
	}

	#[test]
	fn linux_pass_on_2_10_2() {
		assert_eq!(status(&build_check(&v("2.10.2"), Platform::Linux)), "pass");
	}

	#[test]
	fn linux_warn_on_2_10_0() {
		assert_eq!(
			status(&build_check(&v("2.10.0"), Platform::Linux)),
			"warning"
		);
	}

	#[test]
	fn linux_warn_on_2_10_1() {
		assert_eq!(
			status(&build_check(&v("2.10.1"), Platform::Linux)),
			"warning"
		);
	}

	#[test]
	fn linux_fail_below_2_10() {
		assert_eq!(status(&build_check(&v("2.9.0"), Platform::Linux)), "fail");
		assert_eq!(status(&build_check(&v("2.8.0"), Platform::Linux)), "fail");
		assert_eq!(status(&build_check(&v("2.0.0"), Platform::Linux)), "fail");
	}

	#[test]
	fn linux_fail_in_unsafe_band() {
		assert_eq!(status(&build_check(&v("2.10.3"), Platform::Linux)), "fail");
		assert_eq!(status(&build_check(&v("2.11.0"), Platform::Linux)), "fail");
		assert_eq!(status(&build_check(&v("2.11.3"), Platform::Linux)), "fail");
	}

	#[test]
	fn linux_pass_at_or_above_2_11_4() {
		assert_eq!(status(&build_check(&v("2.11.4"), Platform::Linux)), "pass");
		assert_eq!(status(&build_check(&v("2.12.0"), Platform::Linux)), "pass");
		assert_eq!(status(&build_check(&v("3.0.0"), Platform::Linux)), "pass");
	}

	#[test]
	fn windows_pass_on_2_10_2() {
		assert_eq!(
			status(&build_check(&v("2.10.2"), Platform::Windows)),
			"pass"
		);
	}

	#[test]
	fn windows_warn_on_2_9_5() {
		// 2.9.5 is below 2.10.2 (warn) but at/above 2.7.6 (no Windows error).
		assert_eq!(
			status(&build_check(&v("2.9.5"), Platform::Windows)),
			"warning"
		);
	}

	#[test]
	fn windows_warn_on_2_8_0() {
		assert_eq!(
			status(&build_check(&v("2.8.0"), Platform::Windows)),
			"warning"
		);
	}

	#[test]
	fn windows_warn_on_2_7_6() {
		assert_eq!(
			status(&build_check(&v("2.7.6"), Platform::Windows)),
			"warning"
		);
	}

	#[test]
	fn windows_fail_below_2_7_6() {
		assert_eq!(status(&build_check(&v("2.7.5"), Platform::Windows)), "fail");
		assert_eq!(status(&build_check(&v("2.0.0"), Platform::Windows)), "fail");
	}

	#[test]
	fn windows_pass_on_unsafe_linux_band() {
		// 2.10.3 is unsafe on Linux but fine on Windows.
		assert_eq!(
			status(&build_check(&v("2.10.3"), Platform::Windows)),
			"pass"
		);
		assert_eq!(
			status(&build_check(&v("2.11.0"), Platform::Windows)),
			"pass"
		);
	}
}
