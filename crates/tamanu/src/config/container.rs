//! Extract bundled config files from a running Tamanu container.
//!
//! On Linux container deployments the application defaults live inside the
//! image at `/app/packages/{central,facility}-server/config/default.json5`,
//! and only operator overrides land on the host (typically under
//! `/etc/tamanu/<version>/`). Reading the host directory alone misses the
//! defaults, so other systems that depend on the merged config see partial
//! values.
//!
//! We sidestep this by asking podman for the contents directly: find the
//! tamanu container, `podman cp` the file out, and parse it.

use std::{
	path::Path,
	sync::{Mutex, OnceLock},
};

use miette::{IntoDiagnostic, Result, miette};
use tracing::{debug, instrument, trace};

#[derive(Debug, Clone)]
pub(super) struct ContainerSource {
	pub container: String,
	pub package: &'static str,
}

/// Inspect locally running containers for a Tamanu API server.
///
/// Names follow the systemd-quadlet template (`tamanu-{type}-api-{N}`), so
/// the type (central vs facility) is recoverable from the name alone — no
/// need to resolve the image or open the container.
///
/// Result is cached for the lifetime of the process: a single `load_config`
/// call needs to probe several files (`default.json5`, `production.json5`,
/// `local.json5`, …) and re-running `podman ps` each time is wasteful.
#[instrument(level = "debug")]
pub(super) fn detect_running() -> Option<ContainerSource> {
	static CACHE: OnceLock<Mutex<Option<ContainerSource>>> = OnceLock::new();
	let cache = CACHE.get_or_init(|| Mutex::new(probe_running()));
	cache.lock().ok()?.clone()
}

fn probe_running() -> Option<ContainerSource> {
	let output = duct::cmd!("podman", "ps", "--format", "{{.Names}}")
		.stderr_null()
		.read()
		.ok()?;

	for name in output.lines().map(str::trim).filter(|n| !n.is_empty()) {
		if let Some(package) = package_from_container_name(name) {
			trace!(?name, ?package, "found running tamanu container");
			return Some(ContainerSource {
				container: name.to_string(),
				package,
			});
		}
	}

	None
}

pub(super) fn package_from_container_name(name: &str) -> Option<&'static str> {
	let rest = name.strip_prefix("tamanu-")?;
	if rest.starts_with("central-") {
		Some("central-server")
	} else if rest.starts_with("facility-") {
		Some("facility-server")
	} else {
		None
	}
}

/// Copy `file` out of the container's bundled config directory.
///
/// Returns the raw bytes; the caller is responsible for parsing.
#[instrument(level = "debug")]
pub(super) fn copy_bundled_config(source: &ContainerSource, file: &str) -> Result<Vec<u8>> {
	let src = format!(
		"{}:/app/packages/{}/config/{}",
		source.container, source.package, file
	);

	let tmp = tempfile::tempdir().into_diagnostic()?;
	let dest = tmp.path().join(file);

	let output = duct::cmd!("podman", "cp", &src, &dest)
		.stderr_capture()
		.stdout_capture()
		.unchecked()
		.run()
		.into_diagnostic()?;

	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
		debug!(status = ?output.status, %stderr, "podman cp failed");
		return Err(miette!("podman cp {src} failed: {stderr}"));
	}

	std::fs::read(&dest).into_diagnostic()
}

/// Best-effort fetch of `file` from the running tamanu container's bundled
/// config directory. Returns `None` when no suitable container is running, or
/// when the requested package doesn't match the running one.
#[instrument(level = "debug")]
pub(super) fn read_bundled_config(
	package: Option<&str>,
	file: &str,
) -> Option<Result<serde_json::Value>> {
	let source = detect_running()?;

	if let Some(want) = package
		&& want != source.package
	{
		trace!(?want, found = ?source.package, "running container is a different package; skipping");
		return None;
	}

	let bytes = match copy_bundled_config(&source, file) {
		Ok(bytes) => bytes,
		Err(err) => {
			debug!(%err, "failed to extract bundled config from container");
			return None;
		}
	};

	let text = match std::str::from_utf8(&bytes) {
		Ok(text) => text,
		Err(err) => return Some(Err(miette!("bundled {file} is not utf-8: {err}"))),
	};

	Some(json5::from_str(text).into_diagnostic())
}

/// Used by the loader so callers don't accidentally invoke the fallback on
/// hosts that aren't structured like a Linux container deployment.
///
/// A Linux container deployment lays its versioned config out under
/// `/etc/tamanu/`. Roots that don't sit under that prefix (Windows installs,
/// dev checkouts) don't get the fallback.
pub(super) fn root_looks_like_container_host(root: &Path) -> bool {
	if !cfg!(target_os = "linux") {
		return false;
	}

	let Ok(canon) = root.canonicalize() else {
		return root.starts_with("/etc/tamanu");
	};
	canon.starts_with("/etc/tamanu")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn package_from_container_name_central() {
		assert_eq!(
			package_from_container_name("tamanu-central-api-1"),
			Some("central-server"),
		);
		assert_eq!(
			package_from_container_name("tamanu-central-tasks"),
			Some("central-server"),
		);
		assert_eq!(
			package_from_container_name("tamanu-central-fhir-refresh"),
			Some("central-server"),
		);
	}

	#[test]
	fn package_from_container_name_facility() {
		assert_eq!(
			package_from_container_name("tamanu-facility-api-1"),
			Some("facility-server"),
		);
		assert_eq!(
			package_from_container_name("tamanu-facility-sync"),
			Some("facility-server"),
		);
	}

	#[test]
	fn package_from_container_name_ignores_non_api_runtimes() {
		// Frontend / patient-portal containers don't ship the server-side
		// `packages/{central,facility}-server` tree, so they're not useful
		// sources for default.json5.
		assert_eq!(package_from_container_name("tamanu-frontend-a"), None);
		assert_eq!(package_from_container_name("tamanu-patientportal"), None);
		assert_eq!(package_from_container_name("caddy"), None);
		assert_eq!(package_from_container_name(""), None);
	}

	#[test]
	#[cfg(target_os = "linux")]
	fn root_looks_like_container_host_etc_tamanu() {
		// Real-world deployment layout. Even if the path doesn't resolve
		// (e.g. tests run on a host that doesn't have tamanu installed), the
		// prefix match still recognises it as a candidate.
		assert!(root_looks_like_container_host(Path::new(
			"/etc/tamanu/current"
		)));
		assert!(root_looks_like_container_host(Path::new("/etc/tamanu")));
	}

	#[test]
	#[cfg(target_os = "linux")]
	fn root_looks_like_container_host_rejects_other_paths() {
		assert!(!root_looks_like_container_host(Path::new("/opt/tamanu")));
		assert!(!root_looks_like_container_host(Path::new(
			"/var/lib/tamanu"
		)));
		assert!(!root_looks_like_container_host(Path::new(
			"/home/dev/tamanu"
		)));
	}

	#[test]
	#[cfg(not(target_os = "linux"))]
	fn root_looks_like_container_host_disabled_off_linux() {
		assert!(!root_looks_like_container_host(Path::new(
			"/etc/tamanu/current"
		)));
	}
}
