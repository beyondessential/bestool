use clap::Parser;
use miette::Result;

use crate::actions::Context;

/// Upgrade (or install) Caddy.
///
/// Downloads the latest Caddy, replaces the installed binary, and restarts the
/// `caddy` service. If Caddy isn't installed yet, it's placed in the standard
/// location along with a basic Caddyfile.txt and registered as a service.
///
/// Windows only: on Linux, Caddy is managed through the package manager.
#[derive(Debug, Clone, Parser)]
pub struct UpgradeArgs {
	/// Version to install.
	#[arg(long, default_value = "latest")]
	pub version: String,

	/// Target to download.
	///
	/// Usually the auto-detected default is fine; in rare cases you may need to
	/// override it.
	#[arg(long)]
	pub target: Option<String>,
}

pub async fn run(args: UpgradeArgs, _ctx: Context) -> Result<()> {
	#[cfg(windows)]
	{
		windows::run(args).await
	}
	#[cfg(not(windows))]
	{
		let _ = args;
		miette::bail!(
			"`caddy upgrade` is only available on Windows; on Linux, Caddy is managed through the package manager"
		)
	}
}

/// Compare two version strings on their first three numeric components, so a
/// WinSW file version like `2.12.0.0` matches a release tag like `2.12.0`.
#[cfg(any(windows, test))]
fn same_version(a: &str, b: &str) -> bool {
	fn parts(v: &str) -> Vec<u64> {
		v.trim_start_matches('v')
			.split('.')
			.take(3)
			.map(|p| p.parse().unwrap_or(0))
			.collect()
	}
	parts(a) == parts(b)
}

#[cfg(test)]
mod tests {
	use super::same_version;

	#[test]
	fn winsw_file_version_matches_release_tag() {
		assert!(same_version("2.12.0.0", "v2.12.0"));
		assert!(same_version("2.12.0.0", "2.12.0"));
	}

	#[test]
	fn different_versions_do_not_match() {
		assert!(!same_version("2.12.0.0", "v2.13.0"));
		assert!(!same_version("2.11.0.0", "v2.12.0"));
	}
}

#[cfg(windows)]
mod windows {
	use std::{
		ffi::OsStr,
		fs,
		path::{Path, PathBuf},
		process::Command,
		thread::sleep,
		time::{Duration, Instant},
	};

	use binstalk_downloader::{
		download::{Download, PkgFmt},
		remote::Url,
	};
	use detect_targets::get_desired_targets;
	use miette::{IntoDiagnostic, Result, WrapErr, bail, miette};
	use tracing::{debug, info, warn};
	use windows_service::{
		service::{ServiceAccess, ServiceState},
		service_manager::{ServiceManager, ServiceManagerAccess},
	};

	use super::{UpgradeArgs, same_version};
	use crate::download::{DownloadSource, client};

	/// Standard install location on Windows.
	const CADDY_DIR: &str = r"C:\Caddy";
	/// Service name (WinSW-wrapped).
	const SERVICE_NAME: &str = "caddy";
	/// WinSW release API (latest stable — the `latest` endpoint excludes
	/// pre-releases). Queried so we know the version before downloading.
	const WINSW_RELEASE_API: &str = "https://api.github.com/repos/winsw/winsw/releases/latest";
	/// The WinSW asset we install (64-bit build).
	const WINSW_ASSET: &str = "WinSW-x64.exe";

	/// How long to wait for the service to stop before replacing the binary.
	const STOP_TIMEOUT: Duration = Duration::from_secs(30);

	const BASIC_CADDYFILE: &str = "\
# Caddy configuration for this server.
# Edit this file, then apply it with:
#   caddy reload --config C:\\Caddy\\Caddyfile.txt --adapter caddyfile
# or restart the service:
#   sc stop caddy && sc start caddy
# Caddyfile docs: https://caddyserver.com/docs/caddyfile

{
\tadmin localhost:2019
}

:80 {
\trespond \"caddy is running\"
}
";

	const WINSW_XML: &str = r#"<service>
  <id>caddy</id>
  <name>Caddy</name>
  <description>Caddy web server, managed by bestool.</description>
  <executable>C:\Caddy\caddy.exe</executable>
  <arguments>run --config C:\Caddy\Caddyfile.txt --adapter caddyfile</arguments>
  <workingdirectory>C:\Caddy</workingdirectory>
  <startmode>Automatic</startmode>
  <onfailure action="restart" delay="10 sec"/>
  <log mode="roll-by-size">
    <sizeThreshold>10240</sizeThreshold>
    <keepFiles>8</keepFiles>
  </log>
</service>
"#;

	pub async fn run(args: UpgradeArgs) -> Result<()> {
		let dir = PathBuf::from(CADDY_DIR);
		let exe = dir.join("caddy.exe");
		fs::create_dir_all(&dir)
			.into_diagnostic()
			.wrap_err_with(|| format!("creating {}", dir.display()))?;

		// Stage the download next to the target so the replace is a same-volume
		// rename; the `.exe` suffix keeps it runnable so we can read its version.
		let staged = dir.join("caddy.new.exe");
		download_caddy(&args.version, args.target.clone(), &staged).await?;

		if exe.is_file() {
			upgrade_existing(&dir, &exe, &staged).await
		} else {
			install_fresh(&dir, &exe, &staged).await
		}
	}

	/// Upgrade an existing install. Both the new caddy and (when we manage the
	/// shim) the latest WinSW are downloaded and compared against what's
	/// installed *before* touching the service, so the stop/restart only
	/// happens when something actually changed — caddy by version string, WinSW
	/// (which has no version command) by binary contents.
	async fn upgrade_existing(dir: &Path, exe: &Path, staged: &Path) -> Result<()> {
		let old_caddy = caddy_version(exe);
		let new_caddy = caddy_version(staged);
		let caddy_changed = old_caddy.is_none() || old_caddy != new_caddy;

		// WinSW: resolve the latest stable release and compare its version to
		// the installed shim's file version, so we only download and replace it
		// (and take downtime) when it actually changed. Best-effort: a
		// release-API hiccup must not block a caddy upgrade.
		let shim = dir.join("caddy-service.exe");
		let shim_present = shim.is_file();
		let staged_shim = dir.join("caddy-service.new.exe");
		let mut shim_update: Option<String> = None;
		if shim_present {
			match winsw_latest_release().await {
				Ok(rel) => {
					let installed = installed_winsw_version(&shim);
					if installed
						.as_deref()
						.is_some_and(|v| same_version(v, &rel.version))
					{
						debug!(version = %rel.version, "WinSW already current");
					} else {
						download_bin(&rel.url, &staged_shim).await?;
						// If the installed version couldn't be read, fall back to
						// a content check so we don't take downtime for an
						// identical shim.
						if installed.is_none() && same_contents(&staged_shim, &shim) {
							debug!("WinSW shim unchanged (by contents)");
							let _ = fs::remove_file(&staged_shim);
						} else {
							info!(old = ?installed, new = %rel.version, "WinSW update staged");
							shim_update = Some(rel.version);
						}
					}
				}
				Err(err) => {
					warn!(%err, "could not check for a WinSW update; leaving the shim as-is")
				}
			}
		}
		let shim_changed = shim_update.is_some();

		// Bring the service config into line with our canonical WinSW XML, so
		// settings and log handling stay uniform across the fleet. Only when we
		// manage the shim, and only when it actually differs. The WinSW shim
		// reads these settings at start, so applying it means a restart.
		let xml_path = dir.join("caddy-service.xml");
		let xml_changed =
			shim_present && fs::read_to_string(&xml_path).ok().as_deref() != Some(WINSW_XML);

		if !caddy_changed && !shim_changed && !xml_changed {
			info!(caddy = ?new_caddy, "caddy, WinSW and service config already current; leaving the service running");
			let _ = fs::remove_file(staged);
			return Ok(());
		}

		info!(
			service = SERVICE_NAME,
			caddy_changed, shim_changed, xml_changed, "stopping caddy to apply updates"
		);
		stop_and_wait(SERVICE_NAME, STOP_TIMEOUT)?;
		if caddy_changed {
			replace_file(staged, exe)?;
		} else {
			let _ = fs::remove_file(staged);
		}
		if shim_changed {
			replace_file(&staged_shim, &shim)?;
		}
		if xml_changed {
			fs::write(&xml_path, WINSW_XML)
				.into_diagnostic()
				.wrap_err_with(|| format!("writing {}", xml_path.display()))?;
			info!(path = ?xml_path, "updated the caddy service config");
		}
		start_service(SERVICE_NAME)?;
		if caddy_changed {
			info!(old = ?old_caddy, new = ?new_caddy, "caddy upgraded");
		}
		if let Some(version) = &shim_update {
			info!(%version, "WinSW shim upgraded");
		}
		info!(service = SERVICE_NAME, "caddy restarted");
		Ok(())
	}

	/// Query a caddy binary's version (the first token of `caddy version`, e.g.
	/// `v2.8.4`), or None if it can't be run.
	fn caddy_version(exe: &Path) -> Option<String> {
		let out = Command::new(exe).arg("version").output().ok()?;
		if !out.status.success() {
			return None;
		}
		String::from_utf8_lossy(&out.stdout)
			.split_whitespace()
			.next()
			.map(str::to_string)
	}

	/// Whether both files exist and have identical contents.
	fn same_contents(a: &Path, b: &Path) -> bool {
		match (fs::read(a), fs::read(b)) {
			(Ok(x), Ok(y)) => x == y,
			_ => false,
		}
	}

	/// Download a Caddy build from the tools host to `dest`.
	async fn download_caddy(version: &str, target: Option<String>, dest: &Path) -> Result<()> {
		let detected_targets = get_desired_targets(target.map(|t| vec![t]));
		let detected_targets = detected_targets.get().await;
		let client = client().await?;
		let host = DownloadSource::Tools.host();

		let mut url = None;
		for target in detected_targets {
			let try_url = host
				.join(&format!(
					"/caddy/{version}/caddy-{target}{ext}?bust={date}",
					ext = if target.contains("windows") { ".exe" } else { "" },
					date = jiff::Timestamp::now(),
				))
				.into_diagnostic()?;
			debug!(url = %try_url, "trying URL");
			if client
				.remote_gettable(try_url.clone())
				.await
				.into_diagnostic()?
			{
				url.replace(try_url);
				break;
			}
		}
		let Some(url) = url else {
			bail!(
				"no caddy {version} build found for {}",
				detected_targets.join(", ")
			);
		};

		info!(%url, path = ?dest, "downloading caddy");
		Download::new(client, url)
			.and_extract(PkgFmt::Bin, dest)
			.await
			.into_diagnostic()?;
		Ok(())
	}

	/// First-time install: place the binary and a basic Caddyfile.txt, then
	/// register the WinSW-wrapped service (without starting it).
	async fn install_fresh(dir: &Path, exe: &Path, staged: &Path) -> Result<()> {
		info!(path = ?exe, "no existing caddy found; installing");
		replace_file(staged, exe)?;

		let caddyfile = dir.join("Caddyfile.txt");
		if caddyfile.exists() {
			info!(path = ?caddyfile, "keeping existing Caddyfile.txt");
		} else {
			fs::write(&caddyfile, BASIC_CADDYFILE)
				.into_diagnostic()
				.wrap_err_with(|| format!("writing {}", caddyfile.display()))?;
			info!(path = ?caddyfile, "wrote a basic Caddyfile.txt");
		}

		let winsw = dir.join("caddy-service.exe");
		let rel = winsw_latest_release().await?;
		info!(version = %rel.version, "installing WinSW");
		download_bin(&rel.url, &winsw).await?;
		let xml = dir.join("caddy-service.xml");
		fs::write(&xml, WINSW_XML)
			.into_diagnostic()
			.wrap_err_with(|| format!("writing {}", xml.display()))?;

		info!("registering the caddy service");
		let status = Command::new(&winsw)
			.arg("install")
			.status()
			.into_diagnostic()
			.wrap_err("running WinSW install")?;
		if !status.success() {
			bail!("WinSW install exited with {status}");
		}
		info!(
			"caddy installed and registered as the '{SERVICE_NAME}' service (not started); start it with `sc start {SERVICE_NAME}`"
		);
		Ok(())
	}

	/// Download the WinSW service shim to `dest`.
	/// A resolved WinSW release: its version and the download URL for our asset.
	struct WinswRelease {
		version: String,
		url: String,
	}

	/// Query the WinSW release API for the latest stable release.
	async fn winsw_latest_release() -> Result<WinswRelease> {
		let client = crate::http::client_builder()
			.build()
			.into_diagnostic()
			.wrap_err("building an HTTP client")?;
		let body: serde_json::Value = client
			.get(WINSW_RELEASE_API)
			.header("Accept", "application/vnd.github+json")
			.send()
			.await
			.into_diagnostic()
			.wrap_err("querying the WinSW release API")?
			.error_for_status()
			.into_diagnostic()?
			.json()
			.await
			.into_diagnostic()
			.wrap_err("parsing the WinSW release API response")?;

		let version = body["tag_name"]
			.as_str()
			.ok_or_else(|| miette!("WinSW release API response has no tag_name"))?
			.trim_start_matches('v')
			.to_string();
		let url = body["assets"]
			.as_array()
			.into_iter()
			.flatten()
			.find(|asset| asset["name"].as_str() == Some(WINSW_ASSET))
			.and_then(|asset| asset["browser_download_url"].as_str())
			.ok_or_else(|| miette!("WinSW release {version} has no {WINSW_ASSET} asset"))?
			.to_string();
		Ok(WinswRelease { version, url })
	}

	/// Read the file version of an installed WinSW shim, or None if it can't be
	/// determined. WinSW has no version command, so we read the PE version
	/// resource via PowerShell.
	fn installed_winsw_version(shim: &Path) -> Option<String> {
		let out = Command::new("powershell")
			.args(["-NoProfile", "-NonInteractive", "-Command"])
			.arg(format!(
				"(Get-Item -LiteralPath '{}').VersionInfo.FileVersion",
				shim.display()
			))
			.output()
			.ok()?;
		if !out.status.success() {
			return None;
		}
		let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
		(!version.is_empty()).then_some(version)
	}

	/// Download a binary from `url` to `dest`.
	async fn download_bin(url: &str, dest: &Path) -> Result<()> {
		let client = client().await?;
		let url = Url::parse(url).into_diagnostic()?;
		info!(%url, path = ?dest, "downloading");
		Download::new(client, url)
			.and_extract(PkgFmt::Bin, dest)
			.await
			.into_diagnostic()?;
		Ok(())
	}

	/// Replace `to` with `from` (same volume). `to` may or may not exist.
	fn replace_file(from: &Path, to: &Path) -> Result<()> {
		if to.exists() {
			fs::remove_file(to)
				.into_diagnostic()
				.wrap_err_with(|| format!("removing old {}", to.display()))?;
		}
		fs::rename(from, to)
			.into_diagnostic()
			.wrap_err_with(|| format!("moving {} to {}", from.display(), to.display()))?;
		Ok(())
	}

	/// Stop the named service and wait until it reports Stopped, so its binary
	/// is no longer locked and can be replaced.
	fn stop_and_wait(name: &str, timeout: Duration) -> Result<()> {
		let manager =
			ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
				.into_diagnostic()
				.wrap_err("connecting to the service manager")?;
		let service = manager
			.open_service(name, ServiceAccess::QUERY_STATUS | ServiceAccess::STOP)
			.into_diagnostic()
			.wrap_err_with(|| format!("opening the {name} service"))?;

		if service.query_status().into_diagnostic()?.current_state != ServiceState::Stopped {
			service
				.stop()
				.into_diagnostic()
				.wrap_err_with(|| format!("stopping the {name} service"))?;
		}

		let deadline = Instant::now() + timeout;
		loop {
			if service.query_status().into_diagnostic()?.current_state == ServiceState::Stopped {
				return Ok(());
			}
			if Instant::now() >= deadline {
				return Err(miette!(
					"the {name} service did not stop within {}s",
					timeout.as_secs()
				));
			}
			sleep(Duration::from_millis(500));
		}
	}

	/// Start the named service.
	fn start_service(name: &str) -> Result<()> {
		let manager =
			ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
				.into_diagnostic()
				.wrap_err("connecting to the service manager")?;
		let service = manager
			.open_service(name, ServiceAccess::START)
			.into_diagnostic()
			.wrap_err_with(|| format!("opening the {name} service"))?;
		service
			.start(&[] as &[&OsStr])
			.into_diagnostic()
			.wrap_err_with(|| format!("starting the {name} service"))?;
		Ok(())
	}
}
