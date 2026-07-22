//! Caddyfile version healthcheck.
//!
//! spec: CHK-CFV
//!
//! Tamanu ships its Windows Caddyfile with a version marker on the literal
//! first line, in the exact form `# tamanu caddyfile v<N>`. Newer Tamanu
//! releases depend on newer Caddyfile revisions, so this check reads that marker
//! and flags a server still running an outdated (or unmarked) Caddyfile.
//!
//! Applies only to a Windows host with a Tamanu deployment; skips elsewhere, and
//! skips when caddy isn't present (no Caddyfile on disk). A present-but-unmarked
//! Caddyfile is a hard fail. Otherwise:
//!   * v>=9                              → pass
//!   * v<9  and Tamanu >= 2.46.0         → fail
//!   * v<9  and Tamanu <  2.46.0         → warn

use node_semver::Version;

use bestool_tamanu::caddy;

use super::CheckContext;
use crate::doctor::check::Check;

const CHECK_NAME: &str = "caddyfile_version";

/// Lowest Caddyfile marker version considered current.
const MIN_VERSION: u32 = 9;

/// Tamanu version from which an outdated Caddyfile is a hard failure rather than
/// a warning: below this, an old Caddyfile still works and only warrants a nudge
/// to upgrade.
const TAMANU_STRICT_FROM: &str = "2.46.0";

pub async fn run(ctx: CheckContext) -> Check {
	// Windows-only: the marked Caddyfile is a Windows deployment convention, and
	// on Linux caddy is managed through the package manager.
	if !cfg!(windows) {
		return Check::skip(
			CHECK_NAME,
			"not a Windows host",
			"the Caddyfile version check applies only to Windows Tamanu servers",
		);
	}

	let path = caddy::caddyfile_path();
	let contents = match std::fs::read_to_string(&path) {
		Ok(contents) => contents,
		Err(err) => {
			// No Caddyfile means caddy isn't present on this host — a precondition
			// the check needs, so it skips rather than reporting on the server.
			return Check::skip(
				CHECK_NAME,
				"caddy not present",
				format!("could not read a Caddyfile at {}: {err}", path.display()),
			);
		}
	};

	let version = parse_caddyfile_version(contents.lines().next().unwrap_or_default());
	let strict_from = Version::parse(TAMANU_STRICT_FROM).expect("TAMANU_STRICT_FROM is valid semver");
	let check = build_check(version, &ctx.tamanu_version, &strict_from);
	match version {
		Some(v) => check.with_detail("caddyfile_version", v),
		None => check,
	}
}

/// Read the Caddyfile version from its first line. The line must be exactly
/// `# tamanu caddyfile v<N>` with `<N>` a non-negative integer and nothing
/// trailing; a trailing carriage-return or whitespace is ignored so a CRLF
/// Windows file still matches. Returns `None` when the line isn't a valid
/// marker.
fn parse_caddyfile_version(first_line: &str) -> Option<u32> {
	first_line
		.trim_end()
		.strip_prefix("# tamanu caddyfile v")?
		.parse()
		.ok()
}

/// Classify a Caddyfile version against the thresholds, given the host's Tamanu
/// version. `None` means the first line wasn't a valid marker.
fn build_check(version: Option<u32>, tamanu_version: &Version, strict_from: &Version) -> Check {
	let Some(version) = version else {
		return Check::fail(
			CHECK_NAME,
			"Caddyfile version missing",
			"the Caddyfile's first line is not a `# tamanu caddyfile v<N>` version marker",
		);
	};

	if version >= MIN_VERSION {
		return Check::pass(CHECK_NAME, format!("Caddyfile v{version}"));
	}

	if tamanu_version >= strict_from {
		Check::fail(
			CHECK_NAME,
			format!("Caddyfile v{version}"),
			format!(
				"Caddyfile v{version} is older than v{MIN_VERSION}, which Tamanu {tamanu_version} requires"
			),
		)
	} else {
		Check::warning(
			CHECK_NAME,
			format!("Caddyfile v{version}"),
			format!(
				"Caddyfile v{version} is older than v{MIN_VERSION}; upgrade it before moving to Tamanu {strict_from}"
			),
		)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn v(s: &str) -> Version {
		Version::parse(s).unwrap()
	}

	fn strict() -> Version {
		v(TAMANU_STRICT_FROM)
	}

	#[test]
	fn parses_bare_marker() {
		assert_eq!(parse_caddyfile_version("# tamanu caddyfile v9"), Some(9));
	}

	#[test]
	fn parses_marker_with_crlf() {
		assert_eq!(parse_caddyfile_version("# tamanu caddyfile v9\r"), Some(9));
	}

	#[test]
	fn parses_multi_digit_version() {
		assert_eq!(parse_caddyfile_version("# tamanu caddyfile v12"), Some(12));
	}

	#[test]
	fn rejects_trailing_content() {
		assert_eq!(parse_caddyfile_version("# tamanu caddyfile v9 beta"), None);
	}

	#[test]
	fn rejects_wrong_prefix() {
		assert_eq!(parse_caddyfile_version("#tamanu caddyfile v9"), None);
		assert_eq!(parse_caddyfile_version("# Tamanu Caddyfile v9"), None);
		assert_eq!(parse_caddyfile_version("# some other comment"), None);
		assert_eq!(parse_caddyfile_version(""), None);
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
	fn missing_marker_fails_regardless_of_tamanu_version() {
		assert_eq!(status(&build_check(None, &v("2.30.0"), &strict())), "fail");
		assert_eq!(status(&build_check(None, &v("2.50.0"), &strict())), "fail");
	}

	#[test]
	fn current_version_passes() {
		assert_eq!(status(&build_check(Some(9), &v("2.50.0"), &strict())), "pass");
		assert_eq!(status(&build_check(Some(9), &v("2.30.0"), &strict())), "pass");
		assert_eq!(
			status(&build_check(Some(10), &v("2.30.0"), &strict())),
			"pass"
		);
	}

	#[test]
	fn outdated_fails_on_new_tamanu() {
		assert_eq!(status(&build_check(Some(8), &v("2.50.0"), &strict())), "fail");
		// The 2.46.0 boundary itself is strict.
		assert_eq!(status(&build_check(Some(8), &v("2.46.0"), &strict())), "fail");
	}

	#[test]
	fn outdated_warns_on_old_tamanu() {
		assert_eq!(
			status(&build_check(Some(8), &v("2.45.9"), &strict())),
			"warning"
		);
		assert_eq!(
			status(&build_check(Some(0), &v("2.0.0"), &strict())),
			"warning"
		);
	}
}
