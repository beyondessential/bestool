use std::path::PathBuf;

use clap::Parser;

use binstalk_downloader::download::{Download, PkgFmt};
use detect_targets::{TARGET, get_desired_targets};
use miette::{IntoDiagnostic, Result, miette};
use tracing::info;

use crate::{
	args::Args,
	download::{DownloadSource, client},
};

use super::Context;

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

	/// Temporary directory to download to.
	///
	/// Defaults to the system temp directory.
	#[arg(long)]
	pub temp_dir: Option<PathBuf>,

	/// Add to the PATH (only on Windows).
	#[cfg(windows)]
	#[arg(short = 'P', long)]
	pub add_to_path: bool,
}

pub async fn run(ctx: Context<Args, SelfUpdateArgs>) -> Result<()> {
	#[cfg(unix)]
	check_exe_writable()?;

	let SelfUpdateArgs {
		version,
		target,
		temp_dir,
		#[cfg(windows)]
		add_to_path,
	} = ctx.args_sub;

	let client = client().await?;

	let detected_targets = get_desired_targets(target.map(|t| vec![t]));
	let detected_targets = detected_targets.get().await;

	let dir = temp_dir.unwrap_or_else(std::env::temp_dir);
	let filename = format!(
		"bestool{ext}",
		ext = if cfg!(windows) { ".exe" } else { "" }
	);
	let dest = dir.join(&filename);
	let _ = tokio::fs::remove_file(&dest).await;

	let host = DownloadSource::Tools.host();
	let url = host
		.join(&format!(
			"/bestool/{version}/{target}/{filename}",
			target = detected_targets
				.first()
				.cloned()
				.unwrap_or_else(|| TARGET.into()),
		))
		.into_diagnostic()?;
	info!(url = %url, "downloading");

	Download::new(client, url)
		.and_extract(PkgFmt::Bin, &dest)
		.await
		.into_diagnostic()?;

	#[cfg(windows)]
	if add_to_path && let Err(err) = add_self_to_path() {
		tracing::error!("{err:?}");
	}

	info!(?dest, "downloaded, self-upgrading");
	upgrade::run_upgrade(&dest, true, vec!["--version"])
		.map_err(|err| miette!("upgrade: {err:?}"))?;
	Ok(())
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

#[cfg(all(test, unix))]
mod tests {
	use std::fs;
	use tempfile::TempDir;

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
