use std::path::{Path, PathBuf};

use clap::Parser;

use binstalk_downloader::download::{DataVerifier as _, Download, PkgFmt};
use detect_targets::{TARGET, get_desired_targets};
use miette::{IntoDiagnostic, Result, miette};
use tracing::info;
#[cfg(all(windows, feature = "alertd"))]
use tracing::warn;

use crate::download::{
	DownloadSource, ReleaseVerifier, client, fetch_latest_version, fetch_release_signature,
	remote_is_newer,
};

use super::Context;

#[cfg(all(windows, feature = "alertd"))]
pub(crate) mod task;

#[cfg(unix)]
fn check_exe_writable() -> Result<()> {
	let exe_path = std::env::current_exe().into_diagnostic()?;
	let exe_dir = exe_path
		.parent()
		.ok_or_else(|| miette!("current exe is not in a directory"))?;

	// Try to check if we can actually write to the directory
	let test_file = exe_dir.join(".bestool_write_test");
	if std::fs::write(&test_file, b"test").is_err() {
		return Err(miette!(
			"Cannot write to executable directory: {}\n\
			Please run with sudo: sudo bestool self-update",
			exe_dir.display()
		));
	}
	let _ = std::fs::remove_file(test_file);

	Ok(())
}

#[cfg(unix)]
pub(crate) fn is_package_manager_install() -> bool {
	std::path::Path::new("/usr/share/doc/bestool/copyright").exists()
}

#[cfg(not(unix))]
pub(crate) fn is_package_manager_install() -> bool {
	false
}

#[cfg(unix)]
fn check_package_manager_install(force: bool) -> Result<()> {
	if is_package_manager_install() && !force {
		return Err(miette!(
			"bestool appears to be installed via a package manager.\n\
			Please use your package manager to update bestool (e.g., 'apt update && apt upgrade bestool').\n\
			If you want to override this and self-update anyway, use: bestool self-update --force"
		));
	}

	Ok(())
}

/// Update this bestool.
///
/// Alias: self
#[derive(Debug, Clone, Parser)]
pub struct SelfUpdateArgs {
	/// Version to update to.
	#[arg(long, default_value = "latest")]
	pub version: String,

	/// Target to download.
	///
	/// Usually the auto-detected default is fine, in rare cases you may need to override it.
	#[arg(long)]
	pub target: Option<String>,

	/// Update from a local file instead of downloading a release.
	///
	/// The file is copied into place over the running binary. When set, version
	/// resolution, download, and signature verification are all skipped, and the
	/// `--version`/`--target` inputs are ignored.
	#[arg(long)]
	pub from_file: Option<PathBuf>,

	/// Temporary directory to download to.
	///
	/// Defaults to the system temp directory.
	#[arg(long)]
	pub temp_dir: Option<PathBuf>,

	/// Add to the PATH (only on Windows).
	#[cfg(windows)]
	#[arg(short = 'P', long)]
	pub add_to_path: bool,

	/// Force reinstall, even if already on the latest version or installed via package manager.
	#[arg(long)]
	pub force: bool,
}

pub async fn run(args: SelfUpdateArgs, _ctx: Context) -> Result<()> {
	// On Windows, when the alert daemon is running, let it own the binary swap
	// and its own restart rather than racing it from this separate process.
	#[cfg(all(windows, feature = "alertd"))]
	if is_alertd_service_running().await {
		return delegate_to_daemon(&args).await;
	}

	#[cfg(unix)]
	{
		check_exe_writable()?;
		check_package_manager_install(args.force)?;
	}

	let SelfUpdateArgs {
		version,
		target,
		temp_dir,
		from_file,
		force,
		#[cfg(windows)]
		add_to_path,
	} = args;

	#[cfg(windows)]
	if add_to_path && let Err(err) = add_self_to_path() {
		tracing::error!("{err:?}");
	}

	let outcome = if let Some(path) = from_file {
		perform_update_from_file(&path).await?
	} else {
		perform_update(&version, target, temp_dir, force).await?
	};

	match outcome {
		UpdateOutcome::AlreadyCurrent { version } => {
			info!(
				version = %version,
				"already on the latest version; use --force to reinstall"
			);
		}
		UpdateOutcome::Updated { from, to } => {
			info!(from = %from, to = %to, "updated bestool");
		}
	}

	Ok(())
}

/// What [`perform_update`] did.
#[derive(Debug, Clone)]
pub(crate) enum UpdateOutcome {
	/// The running build is already current; nothing was downloaded.
	AlreadyCurrent { version: String },
	/// The running binary was replaced; `to` is now installed.
	Updated { from: String, to: String },
}

/// Resolve, download, verify, and install a bestool release, replacing the
/// running binary in place.
///
/// `version` is `"latest"` or a specific version. With `"latest"` and `force`
/// unset, returns [`UpdateOutcome::AlreadyCurrent`] without downloading when the
/// running build is already at or ahead of the published release. A specific
/// version is always installed.
///
/// The downloaded archive is verified against the embedded release public key
/// before the binary it contains is swapped in; an artifact with a missing or
/// invalid signature is never installed. Replacing the binary does not restart
/// anything — the caller decides what happens next (a CLI invocation exits; the
/// daemon requests its own restart).
pub(crate) async fn perform_update(
	version: &str,
	target: Option<String>,
	temp_dir: Option<PathBuf>,
	force: bool,
) -> Result<UpdateOutcome> {
	let current = env!("CARGO_PKG_VERSION");

	let resolved = if version == "latest" {
		let latest = fetch_latest_version().await?;
		if !force && !remote_is_newer(current, &latest) {
			return Ok(UpdateOutcome::AlreadyCurrent {
				version: current.to_string(),
			});
		}
		latest
	} else {
		version.to_string()
	};

	info!(from = current, to = %resolved, "updating bestool");

	let client = client().await?;

	let detected_targets = get_desired_targets(target.map(|t| vec![t]));
	let mut candidates = detected_targets.get().await.to_vec();
	if candidates.is_empty() {
		candidates.push(TARGET.into());
	}

	let dir = temp_dir.unwrap_or_else(std::env::temp_dir);
	let filename = format!(
		"bestool{ext}",
		ext = if cfg!(windows) { ".exe" } else { "" }
	);
	let dest = dir.join(&filename);
	let _ = tokio::fs::remove_file(&dest).await;

	// Try each detected target in order until one has artifacts: on a glibc
	// host detection lists the gnu triple before musl, and Linux releases are
	// musl-only, so the gnu candidate can 404.
	let host = DownloadSource::Tools.host();
	let mut signature = None;
	let mut archive_url = None;
	for target in &candidates {
		let archive_path = format!("/bestool/{resolved}/{target}/{filename}.tar.zst");
		let url = host.join(&archive_path).into_diagnostic()?;
		match fetch_release_signature(&client, &url).await {
			Ok(sig) => {
				signature = Some(sig);
				archive_url = Some(url);
				break;
			}
			Err(err) => info!(%target, "no release artifact for target: {err}"),
		}
	}
	let (Some(signature), Some(archive_url)) = (signature, archive_url) else {
		return Err(miette!(
			"no release artifact found for {resolved} (tried targets: {})",
			candidates.join(", ")
		));
	};

	info!(url = %archive_url, "downloading and verifying release");
	let mut verifier = ReleaseVerifier::new(signature);
	Download::new_with_data_verifier(client, archive_url, &mut verifier)
		.and_extract(PkgFmt::Tzstd, &dir)
		.await
		.into_diagnostic()?;

	if !verifier.validate() {
		return Err(miette!(
			"release signature verification failed; refusing to install {resolved}"
		));
	}

	info!(?dest, "signature verified, replacing binary");
	self_replace::self_replace(&dest).into_diagnostic()?;
	let _ = tokio::fs::remove_file(&dest).await;

	Ok(UpdateOutcome::Updated {
		from: current.to_string(),
		to: resolved,
	})
}

/// Replace the running binary in place with a local file supplied by the
/// operator, without downloading or verifying anything.
///
/// The signature check that guards the download path is deliberately skipped:
/// the file is an explicit operator-supplied local binary, analogous to
/// `--force`. The operator's file is left in place — only the running binary is
/// overwritten. Replacing the binary does not restart anything; the caller
/// decides what happens next.
pub(crate) async fn perform_update_from_file(path: &Path) -> Result<UpdateOutcome> {
	let current = env!("CARGO_PKG_VERSION");

	if !path.is_file() {
		return Err(miette!(
			"cannot update from {}: not an existing file",
			path.display()
		));
	}

	info!(from = %path.display(), "replacing binary from local file");
	self_replace::self_replace(path).into_diagnostic()?;

	Ok(UpdateOutcome::Updated {
		from: current.to_string(),
		to: format!("file:{}", path.display()),
	})
}

/// Ask the running alert daemon to perform the update (and restart itself),
/// rather than swapping the binary from this separate process.
///
/// Surfaces the daemon's decision (updating, or already current) to the
/// operator who ran the command.
#[cfg(all(windows, feature = "alertd"))]
async fn delegate_to_daemon(args: &SelfUpdateArgs) -> Result<()> {
	use serde_json::Value;

	// Reuse the daemon client that probes every default address (v6 and v4
	// loopback) and returns the base URL that answered: the daemon binds only
	// the first address it can, so a hardcoded family can miss it.
	let (client, base_url) =
		bestool_alertd::commands::try_connect_daemon(&bestool_alertd::commands::default_server_addrs())
			.await?;

	let mut url = reqwest::Url::parse(&format!("{base_url}/tasks/self-update/update"))
		.into_diagnostic()?;
	url.query_pairs_mut()
		.append_pair("version", &args.version)
		.append_pair("force", if args.force { "true" } else { "false" });

	if let Some(path) = &args.from_file {
		// The daemon runs as LocalSystem with a different working directory, so
		// a relative path won't resolve there: hand it an absolute one.
		let absolute = std::fs::canonicalize(path)
			.map_err(|err| miette!("could not resolve --from-file path {}: {err}", path.display()))?;
		let absolute = absolute
			.to_str()
			.ok_or_else(|| miette!("--from-file path is not valid UTF-8: {}", absolute.display()))?;
		url.query_pairs_mut().append_pair("from_file", absolute);
	}

	info!("alert daemon is running; delegating update to it");

	// Record the daemon's start marker before it updates, so we can tell its
	// eventual restart (a fresh process) apart from the process we're talking to.
	let previous_started_at = fetch_daemon_status(&client, &base_url)
		.await
		.map(|status| status.started_at);

	let response = client
		.get(url)
		.send()
		.await
		.map_err(|err| miette!("could not reach the alert daemon: {err}"))?;

	let body: Value = response
		.json()
		.await
		.map_err(|err| miette!("unexpected response from the alert daemon: {err}"))?;

	if body.get("updating").and_then(Value::as_bool) == Some(true) {
		let from = body.get("from").and_then(Value::as_str).unwrap_or("?");
		let to = body
			.get("to")
			.and_then(Value::as_str)
			.unwrap_or("?")
			.to_string();
		info!(from, to = %to, "alert daemon is updating and will restart");
		wait_for_daemon_restart(&client, &base_url, previous_started_at.as_deref(), &to).await?;
	} else if let Some(message) = body.get("error").and_then(Value::as_str) {
		return Err(miette!("alert daemon could not update: {message}"));
	} else {
		let current = body.get("current").and_then(Value::as_str).unwrap_or("?");
		info!(version = current, "alert daemon is already on the latest version");
	}

	Ok(())
}

/// Fetch and parse the daemon's `/status`, or `None` if it's unreachable or the
/// response can't be parsed (both expected while it's mid-restart).
#[cfg(all(windows, feature = "alertd"))]
async fn fetch_daemon_status(
	client: &reqwest::Client,
	base_url: &str,
) -> Option<bestool_alertd::http_server::StatusResponse> {
	let response = client
		.get(format!("{base_url}/status"))
		.send()
		.await
		.ok()?;
	if !response.status().is_success() {
		return None;
	}
	response.json().await.ok()
}

/// Fetch the version, if any, that the daemon's self-update task last failed to
/// install, via `/tasks/self-update/status`.
#[cfg(all(windows, feature = "alertd"))]
async fn fetch_update_failure(client: &reqwest::Client, base_url: &str) -> Option<String> {
	use serde_json::Value;

	let response = client
		.get(format!("{base_url}/tasks/self-update/status"))
		.send()
		.await
		.ok()?;
	if !response.status().is_success() {
		return None;
	}
	let body: Value = response.json().await.ok()?;
	body.get("failed_version")
		.and_then(Value::as_str)
		.map(str::to_owned)
}

/// Whether a freshly-observed `/status` means the delegated update has landed:
/// a changed `started_at` marks a fresh process. Without a baseline (the
/// pre-update status couldn't be read) we fall back to the running version
/// matching the requested `target` (or any restart for a `file:` target, whose
/// installed version we can't predict).
#[cfg(all(windows, feature = "alertd"))]
fn restart_completed(
	previous_started_at: Option<&str>,
	current_started_at: &str,
	current_version: &str,
	target: &str,
) -> bool {
	match previous_started_at {
		Some(previous) => current_started_at != previous,
		None => target.starts_with("file:") || current_version == target,
	}
}

/// Block until the daemon has installed the delegated update and restarted, so
/// the operator sees the update through to completion instead of the command
/// exiting the moment the daemon accepts it.
#[cfg(all(windows, feature = "alertd"))]
async fn wait_for_daemon_restart(
	client: &reqwest::Client,
	base_url: &str,
	previous_started_at: Option<&str>,
	target: &str,
) -> Result<()> {
	use std::time::{Duration, Instant};

	const TIMEOUT: Duration = Duration::from_secs(300);
	const POLL_INTERVAL: Duration = Duration::from_secs(2);

	let deadline = Instant::now() + TIMEOUT;
	let target_is_file = target.starts_with("file:");

	info!("waiting for the alert daemon to install the update and restart");
	loop {
		// A recorded failure means the daemon we're polling stayed up and won't
		// restart: surface it rather than waiting out the timeout.
		if fetch_update_failure(client, base_url).await.as_deref() == Some(target) {
			return Err(miette!(
				"alert daemon reported that updating to {target} failed; \
				check the daemon logs (`bestool alertd status`) for details"
			));
		}

		if let Some(status) = fetch_daemon_status(client, base_url).await
			&& restart_completed(
				previous_started_at,
				&status.started_at,
				&status.version,
				target,
			) {
			if !target_is_file && status.version != target {
				warn!(
					expected = %target,
					running = %status.version,
					"alert daemon restarted but is not on the expected version"
				);
			} else {
				info!(version = %status.version, "alert daemon updated and restarted");
			}
			return Ok(());
		}

		if Instant::now() >= deadline {
			return Err(miette!(
				"timed out after {}s waiting for the alert daemon to restart on the new version; \
				check `bestool alertd status` and the daemon logs",
				TIMEOUT.as_secs()
			));
		}

		tokio::time::sleep(POLL_INTERVAL).await;
	}
}

#[cfg(windows)]
fn add_self_to_path() -> Result<()> {
	let self_path = std::env::current_exe().into_diagnostic()?;
	let self_dir = self_path
		.parent()
		.ok_or_else(|| miette!("current exe is not in a dir?"))?;
	let self_dir = self_dir
		.to_str()
		.ok_or_else(|| miette!("current exe path is not utf-8"))?;

	windows_env::prepend("PATH", self_dir).into_diagnostic()?;

	Ok(())
}

#[cfg(all(windows, feature = "alertd"))]
async fn is_alertd_service_running() -> bool {
	// Probes every default address (v6 and v4 loopback): the daemon binds only
	// the first address it can, so checking a single family can miss it.
	bestool_alertd::commands::try_connect_daemon(&bestool_alertd::commands::default_server_addrs())
		.await
		.is_ok()
}

#[cfg(all(test, unix))]
mod tests {
	use std::fs;
	use tempfile::TempDir;

	use super::perform_update_from_file;

	#[tokio::test]
	async fn from_file_missing_path_errors_without_replacing() {
		// A path that does not exist must be rejected before self_replace is
		// called, so this test never swaps the running test binary.
		let temp_dir = TempDir::new().unwrap();
		let missing = temp_dir.path().join("does-not-exist");
		assert!(!missing.exists());

		let result = perform_update_from_file(&missing).await;
		assert!(result.is_err());
	}

	#[test]
	fn test_check_exe_writable_with_writable_dir() {
		// This test checks that the function succeeds when the exe is in a writable directory
		// We can't easily test this in a hermetic way since check_exe_writable() uses current_exe(),
		// but we can verify the logic by checking that a writable temp directory works
		let temp_dir = TempDir::new().unwrap();
		let test_file = temp_dir.path().join(".bestool_write_test");

		// Should succeed
		assert!(fs::write(&test_file, b"test").is_ok());
		assert!(fs::remove_file(test_file).is_ok());
	}

	#[test]
	fn test_check_exe_writable_with_readonly_dir() {
		// This test verifies that write attempts fail on read-only directories
		use std::os::unix::fs::PermissionsExt;

		let temp_dir = TempDir::new().unwrap();
		let temp_path = temp_dir.path();

		// Make directory read-only
		let mut perms = fs::metadata(temp_path).unwrap().permissions();
		perms.set_mode(0o555);
		fs::set_permissions(temp_path, perms).unwrap();

		let test_file = temp_path.join(".bestool_write_test");

		// Should fail
		assert!(fs::write(&test_file, b"test").is_err());

		// Restore permissions for cleanup
		let mut perms = fs::metadata(temp_path).unwrap().permissions();
		perms.set_mode(0o755);
		let _ = fs::set_permissions(temp_path, perms);
	}
}

#[cfg(all(test, windows, feature = "alertd"))]
mod delegate_tests {
	use super::restart_completed;

	#[test]
	fn changed_started_at_means_restarted() {
		assert!(restart_completed(
			Some("2026-07-20T01:00:00Z"),
			"2026-07-20T01:03:00Z",
			"1.46.1",
			"1.46.1",
		));
	}

	#[test]
	fn same_started_at_means_still_running() {
		assert!(!restart_completed(
			Some("2026-07-20T01:00:00Z"),
			"2026-07-20T01:00:00Z",
			"1.36.2",
			"1.46.1",
		));
	}

	#[test]
	fn without_baseline_matches_target_version() {
		assert!(restart_completed(None, "whenever", "1.46.1", "1.46.1"));
		assert!(!restart_completed(None, "whenever", "1.36.2", "1.46.1"));
	}

	#[test]
	fn without_baseline_any_restart_for_file_target() {
		assert!(restart_completed(
			None,
			"whenever",
			"1.36.2",
			"file:C:\\tmp\\bestool.exe",
		));
	}
}
