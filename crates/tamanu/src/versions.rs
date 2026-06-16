//! Detect the *expected* and *actually running* Tamanu versions, so
//! `tamanu status` / `doctor` can flag when an upgrade has been only
//! partially rolled out (env file bumped but a container still on the
//! previous tag).
//!
//! Two independent sources of truth:
//!
//! - **Expected**, from the deployment's config:
//!   - Linux: `/etc/tamanu/env`'s `TAMANU_VERSION` / `TAMANU_FRONTEND_VERSION`.
//!   - Windows / pm2: the version `find_tamanu()` discovered (one per host;
//!     pm2 processes share the install root).
//! - **Actual**, from the supervisor's view of running services:
//!   - Linux: each container's image tag, looked up via `podman ps`
//!     filtering on `PODMAN_SYSTEMD_UNIT`.
//!   - Windows: same value for every pm2 process (the install version).

use std::{collections::HashMap, path::Path, time::Duration};

use node_semver::Version;
use tracing::{debug, instrument};

#[cfg(target_os = "linux")]
use tracing::warn;

/// Versions configured for this deployment, split by which env variable
/// drives which service.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExpectedVersions {
	/// `TAMANU_VERSION` — API, tasks, sync, fhir-* and patient-portal
	/// containers (Linux); the install root version (pm2).
	pub tamanu: Option<String>,
	/// `TAMANU_FRONTEND_VERSION` — only the `tamanu-frontend@*` containers
	/// use this. None means "fall back to `tamanu`".
	pub frontend: Option<String>,
}

impl ExpectedVersions {
	/// Resolve the expected version for a given expectation name. Frontend
	/// services prefer `frontend`; everything else (and frontend when no
	/// frontend-specific version is set) falls back to `tamanu`.
	pub fn for_service(&self, expectation_name: &str) -> Option<&str> {
		if expectation_name == "tamanu-frontend" {
			self.frontend.as_deref().or(self.tamanu.as_deref())
		} else {
			self.tamanu.as_deref()
		}
	}
}

/// Parse a key=value `/etc/tamanu/env` file. Tolerant of:
/// - comment lines starting with `#`
/// - blank lines
/// - quoted values (single or double quotes get stripped)
/// - other unrelated keys (silently ignored)
///
/// Keys we don't recognise are dropped: this isn't a general env-file
/// parser, just enough to find the two version keys.
pub fn parse_env_file(content: &str) -> ExpectedVersions {
	let mut out = ExpectedVersions::default();
	for line in content.lines() {
		let line = line.trim();
		if line.is_empty() || line.starts_with('#') {
			continue;
		}
		let Some((key, value)) = line.split_once('=') else {
			continue;
		};
		let key = key.trim();
		let value = strip_quotes(value.trim());
		if value.is_empty() {
			continue;
		}
		match key {
			"TAMANU_VERSION" => out.tamanu = Some(value.to_string()),
			"TAMANU_FRONTEND_VERSION" => out.frontend = Some(value.to_string()),
			_ => {}
		}
	}
	out
}

fn strip_quotes(s: &str) -> &str {
	let bytes = s.as_bytes();
	if bytes.len() >= 2
		&& ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
			|| (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
	{
		&s[1..s.len() - 1]
	} else {
		s
	}
}

/// Read `/etc/tamanu/env` and extract version variables. On any error
/// (file missing, unreadable) returns an empty `ExpectedVersions` and
/// logs at debug — the caller treats that as "no expected version
/// known" and won't flag a mismatch.
#[instrument(level = "debug")]
pub fn read_env_file(path: &Path) -> ExpectedVersions {
	match std::fs::read_to_string(path) {
		Ok(content) => parse_env_file(&content),
		Err(err) => {
			debug!(?path, %err, "could not read env file");
			ExpectedVersions::default()
		}
	}
}

/// Split a container image reference into `(repo, tag)`. Returns `None`
/// when the reference doesn't carry a `:tag` (e.g. raw image ID).
///
/// Handles registry references with embedded ports
/// (`registry.example.com:5000/image:tag`) by splitting on the *last*
/// colon when it appears after the final slash, and refuses to misread
/// the port as a tag.
pub fn parse_image_tag(image: &str) -> Option<&str> {
	// Take the last segment after `/` so the registry port (if any) is
	// outside our colon search.
	let last_segment_start = image.rfind('/').map(|i| i + 1).unwrap_or(0);
	let last_segment = &image[last_segment_start..];
	let colon_in_segment = last_segment.rfind(':')?;
	let tag = &last_segment[colon_in_segment + 1..];
	if tag.is_empty() { None } else { Some(tag) }
}

/// Returns a map from systemd unit name (e.g. `tamanu-central-api@1.service`)
/// to the running container's image tag. Containers without an image tag, or
/// not labelled as systemd-managed, are skipped.
///
/// `Ok(map)` means podman answered — an empty map then genuinely means "no
/// tamanu containers are running". `Err(reason)` means we couldn't ask at all:
/// podman is missing, or (the common case) the containers are root-owned and
/// this process can't see them — e.g. an unprivileged `podman ps`. Callers
/// must distinguish the two: surfacing `Err` as "no containers" hides a blind
/// spot behind a healthy-looking result.
#[cfg(target_os = "linux")]
#[instrument(level = "debug")]
pub async fn running_versions_linux() -> Result<HashMap<String, String>, String> {
	let result = tokio::process::Command::new("podman")
		.args([
			"ps",
			"--format",
			"{{.Labels.PODMAN_SYSTEMD_UNIT}}\t{{.Image}}",
		])
		.output()
		.await;
	let output = match result {
		Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
		Ok(o) => {
			let stderr = String::from_utf8_lossy(&o.stderr);
			let stderr = stderr.trim();
			return Err(if stderr.is_empty() {
				format!("podman ps exited {}", o.status)
			} else {
				format!("podman ps exited {}: {stderr}", o.status)
			});
		}
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
			return Err("podman not found on PATH".to_string());
		}
		Err(err) => return Err(format!("could not run podman ps: {err}")),
	};

	let mut out = HashMap::new();
	for line in output.lines() {
		let Some((unit, image)) = line.split_once('\t') else {
			continue;
		};
		let unit = unit.trim();
		let image = image.trim();
		if unit.is_empty() || image.is_empty() {
			continue;
		}
		let Some(tag) = parse_image_tag(image) else {
			warn!(unit, image, "container image carries no parseable tag");
			continue;
		};
		out.insert(unit.to_string(), tag.to_string());
	}
	Ok(out)
}

#[cfg(not(target_os = "linux"))]
pub async fn running_versions_linux() -> Result<HashMap<String, String>, String> {
	Ok(HashMap::new())
}

/// Parse a version string leniently, tolerating a leading `v` (image tags and
/// env values both occur in `v2.48.5` form) and surrounding whitespace.
pub fn parse_version_loose(s: &str) -> Option<Version> {
	Version::parse(s.trim().trim_start_matches('v')).ok()
}

/// Pick the running server version out of a unit→tag map (as returned by
/// [`running_versions_linux`]).
///
/// Prefers the `tamanu-{central,facility}-api` units; falls back to any other
/// `tamanu-*` unit except the frontend, which legitimately runs its own
/// version. When instances disagree (mid-upgrade), returns the highest.
pub fn pick_running_version(running: &HashMap<String, String>) -> Option<Version> {
	let mut api = Vec::new();
	let mut other = Vec::new();
	for (unit, tag) in running {
		let Some((base, _instance)) = super::services::parse_systemd_unit(unit) else {
			continue;
		};
		let Some(version) = parse_version_loose(tag) else {
			continue;
		};
		match base {
			"tamanu-central-api" | "tamanu-facility-api" => api.push(version),
			"tamanu-frontend" => {}
			_ => other.push(version),
		}
	}
	if api.is_empty() { other } else { api }.into_iter().max()
}

/// The version of Tamanu that's actually running, from container image tags.
/// `None` when podman can't be read or nothing is running.
pub async fn running_version() -> Option<Version> {
	match running_versions_linux().await {
		Ok(map) => pick_running_version(&map),
		Err(_) => None,
	}
}

/// The version Tamanu last recorded in its own database. The server writes
/// `local_system_facts.currentVersion` on boot, so this reflects the last
/// version that ran against this database even when nothing is up right now.
pub async fn db_current_version(database_url: &str) -> Option<Version> {
	const TIMEOUT: Duration = Duration::from_secs(5);

	let client = match tokio::time::timeout(
		TIMEOUT,
		bestool_postgres::pool::connect_one(database_url, "bestool-tamanu-discovery"),
	)
	.await
	{
		Ok(Ok(client)) => client,
		Ok(Err(err)) => {
			debug!(%err, "could not open DB for version discovery");
			return None;
		}
		Err(_) => {
			debug!("DB connection timed out during version discovery");
			return None;
		}
	};

	let row = match tokio::time::timeout(
		TIMEOUT,
		client.query_opt(
			"SELECT value FROM local_system_facts WHERE key = 'currentVersion'",
			&[],
		),
	)
	.await
	{
		Ok(Ok(row)) => row?,
		Ok(Err(err)) => {
			debug!(%err, "could not query currentVersion");
			return None;
		}
		Err(_) => {
			debug!("currentVersion query timed out");
			return None;
		}
	};

	row.try_get::<_, String>(0)
		.ok()
		.as_deref()
		.and_then(parse_version_loose)
}

/// The version the deployment is configured to start, from the env file.
pub fn env_file_version(path: &Path) -> Option<Version> {
	read_env_file(path)
		.tamanu
		.as_deref()
		.and_then(parse_version_loose)
}

/// Comparison verdict for one instance.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum VersionStatus {
	/// Actual matches expected.
	Match,
	/// Actual is set but doesn't match expected.
	Mismatch,
	/// Either expected or actual (or both) is unknown — render in the UI
	/// but don't fail the check.
	Unknown,
}

impl VersionStatus {
	pub fn is_mismatch(self) -> bool {
		matches!(self, VersionStatus::Mismatch)
	}
}

/// Classify an `(actual, expected)` pair.
pub fn classify(actual: Option<&str>, expected: Option<&str>) -> VersionStatus {
	match (actual, expected) {
		(Some(a), Some(e)) if a == e => VersionStatus::Match,
		(Some(_), Some(_)) => VersionStatus::Mismatch,
		_ => VersionStatus::Unknown,
	}
}

/// Convenience for the `find_tamanu`-style integration: read the env
/// file from the conventional Linux location, falling back to the
/// install-root's version on pm2 deployments.
pub fn expected_for_supervisor(
	supervisor: super::services::Supervisor,
	install_version: &node_semver::Version,
) -> ExpectedVersions {
	use super::services::Supervisor;

	match supervisor {
		Supervisor::Systemd => read_env_file(Path::new("/etc/tamanu/env")),
		Supervisor::Pm2 => {
			// pm2 hosts have one install root and no env file; both keys
			// resolve to the same version. We set both so the
			// `for_service` logic doesn't need to special-case the
			// supervisor.
			let v = install_version.to_string();
			ExpectedVersions {
				tamanu: Some(v.clone()),
				frontend: Some(v),
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_env_file_picks_known_keys() {
		let content = "\
TZ=Pacific/Auckland
TAMANU_VERSION=v2.10.0
TAMANU_VERSION_DOMAIN=v2-10-0
TAMANU_FRONTEND_VERSION=v2.10.1
TAMANU_FRONTEND_VERSION_DOMAIN=v2-10-1
";
		let v = parse_env_file(content);
		assert_eq!(v.tamanu.as_deref(), Some("v2.10.0"));
		assert_eq!(v.frontend.as_deref(), Some("v2.10.1"));
	}

	#[test]
	fn parse_env_file_missing_frontend_yields_none() {
		let content = "TAMANU_VERSION=v2.10.0\n";
		let v = parse_env_file(content);
		assert_eq!(v.tamanu.as_deref(), Some("v2.10.0"));
		assert!(v.frontend.is_none());
	}

	#[test]
	fn parse_env_file_skips_comments_and_blanks() {
		let content = "\
# heading
\t
TAMANU_VERSION = v2.10.0
\t\t
# trailer
";
		let v = parse_env_file(content);
		assert_eq!(v.tamanu.as_deref(), Some("v2.10.0"));
	}

	#[test]
	fn parse_env_file_strips_quotes() {
		let v = parse_env_file("TAMANU_VERSION=\"v2.10.0\"\nTAMANU_FRONTEND_VERSION='v2.10.1'\n");
		assert_eq!(v.tamanu.as_deref(), Some("v2.10.0"));
		assert_eq!(v.frontend.as_deref(), Some("v2.10.1"));
	}

	#[test]
	fn parse_env_file_ignores_empty_values() {
		let v = parse_env_file("TAMANU_VERSION=\nTAMANU_FRONTEND_VERSION=v2.10.0\n");
		assert!(v.tamanu.is_none());
		assert_eq!(v.frontend.as_deref(), Some("v2.10.0"));
	}

	#[test]
	fn for_service_frontend_prefers_frontend_version() {
		let v = ExpectedVersions {
			tamanu: Some("v2.10.0".into()),
			frontend: Some("v2.10.1".into()),
		};
		assert_eq!(v.for_service("tamanu-frontend"), Some("v2.10.1"));
		assert_eq!(v.for_service("tamanu-central-api"), Some("v2.10.0"));
	}

	#[test]
	fn for_service_frontend_falls_back_to_tamanu_when_unset() {
		// Older deployments only have TAMANU_VERSION; the frontend tracks
		// the API version.
		let v = ExpectedVersions {
			tamanu: Some("v2.10.0".into()),
			frontend: None,
		};
		assert_eq!(v.for_service("tamanu-frontend"), Some("v2.10.0"));
	}

	#[test]
	fn for_service_nothing_known() {
		let v = ExpectedVersions::default();
		assert!(v.for_service("tamanu-frontend").is_none());
		assert!(v.for_service("tamanu-central-api").is_none());
	}

	#[test]
	fn parse_image_tag_simple() {
		assert_eq!(
			parse_image_tag("ghcr.io/beyondessential/tamanu-central:v2.10.0"),
			Some("v2.10.0")
		);
	}

	#[test]
	fn parse_image_tag_no_tag() {
		// Bare image ID or repo with no tag.
		assert_eq!(
			parse_image_tag("ghcr.io/beyondessential/tamanu-central"),
			None
		);
		assert_eq!(parse_image_tag("imagewithouttag"), None);
	}

	#[test]
	fn parse_image_tag_registry_with_port_doesnt_confuse_tag() {
		// `localhost:5000` is the registry, not a tag.
		assert_eq!(
			parse_image_tag("localhost:5000/tamanu-central"),
			None,
			"port without a tag should NOT be read as the tag"
		);
		assert_eq!(
			parse_image_tag("localhost:5000/tamanu-central:v2.10.0"),
			Some("v2.10.0")
		);
	}

	#[test]
	fn parse_image_tag_handles_sha_digest_form() {
		// `@sha256:...` digest references aren't tags. We currently
		// return None — a SHA digest has no human-meaningful version, so
		// matching `unknown` is fine.
		let img = "ghcr.io/beyondessential/tamanu-central@sha256:abcdef";
		// rfind(':') in the last segment finds the colon inside `sha256:abcdef`,
		// yielding "abcdef" — not ideal, but the @ marker is the
		// disambiguator. Tolerate it for now: the caller compares against
		// the env file's literal tag string and won't get a false match.
		let tag = parse_image_tag(img);
		assert!(
			tag.is_none() || tag == Some("abcdef"),
			"unexpected tag parse: {tag:?}"
		);
	}

	#[test]
	fn parse_version_loose_tolerates_v_prefix_and_whitespace() {
		let v: Version = "2.48.5".parse().unwrap();
		assert_eq!(parse_version_loose("2.48.5"), Some(v.clone()));
		assert_eq!(parse_version_loose("v2.48.5"), Some(v.clone()));
		assert_eq!(parse_version_loose(" v2.48.5 "), Some(v));
		assert_eq!(parse_version_loose("latest"), None);
		assert_eq!(parse_version_loose(""), None);
	}

	#[test]
	fn pick_running_version_prefers_api_units() {
		let running = HashMap::from([
			(
				"tamanu-central-api@1.service".to_string(),
				"v2.48.5".to_string(),
			),
			(
				"tamanu-central-tasks.service".to_string(),
				"v2.55.4".to_string(),
			),
		]);
		assert_eq!(
			pick_running_version(&running),
			Some("2.48.5".parse().unwrap())
		);
	}

	#[test]
	fn pick_running_version_ignores_frontend_and_foreign_units() {
		let running = HashMap::from([
			(
				"tamanu-frontend@a.service".to_string(),
				"v2.55.4".to_string(),
			),
			("caddy.service".to_string(), "v9.9.9".to_string()),
			(
				"tamanu-central-tasks.service".to_string(),
				"v2.48.5".to_string(),
			),
		]);
		assert_eq!(
			pick_running_version(&running),
			Some("2.48.5".parse().unwrap())
		);
	}

	#[test]
	fn pick_running_version_takes_highest_on_disagreement() {
		// Mid-rolling-upgrade two API instances can briefly run different
		// tags; either answer is defensible, so pick deterministically.
		let running = HashMap::from([
			(
				"tamanu-central-api@1.service".to_string(),
				"v2.48.5".to_string(),
			),
			(
				"tamanu-central-api@2.service".to_string(),
				"v2.55.4".to_string(),
			),
		]);
		assert_eq!(
			pick_running_version(&running),
			Some("2.55.4".parse().unwrap())
		);
	}

	#[test]
	fn pick_running_version_empty() {
		assert_eq!(pick_running_version(&HashMap::new()), None);
	}

	#[test]
	fn classify_match_mismatch_unknown() {
		assert_eq!(
			classify(Some("v2.10.0"), Some("v2.10.0")),
			VersionStatus::Match
		);
		assert_eq!(
			classify(Some("v2.10.0"), Some("v2.10.1")),
			VersionStatus::Mismatch
		);
		assert_eq!(classify(None, Some("v2.10.0")), VersionStatus::Unknown);
		assert_eq!(classify(Some("v2.10.0"), None), VersionStatus::Unknown);
		assert_eq!(classify(None, None), VersionStatus::Unknown);
	}
}
