//! Source preparation for the `simple` method.
//!
//! `simple` backs up an arbitrary path whose files may be owned by users the
//! kopia user can't read (e.g. postgres' `pg_hba.conf`, mode 0640). So the
//! privileged daemon first exposes a kopia-readable *view* of the source, then
//! kopia snapshots that view unprivileged:
//!   - `bindfs --force-user=kopia` presents the whole tree as kopia-owned — no
//!     copy, any filesystem, any ownership. Preferred.
//!   - failing that (bindfs absent or unusable), a root copy chowned to the
//!     kopia user: a universal but heavier fallback.
//!
//! The view lives under the daemon's `CacheDirectory` (`/var/cache/bestool`,
//! root-owned and world-traversable) so root can create it and the kopia user
//! can still reach the view inside it. On non-Linux there's no kopia user —
//! kopia runs as the current user and snapshots the source in place.

use std::path::{Path, PathBuf};

use miette::Result;
#[cfg(target_os = "linux")]
use miette::{Context as _, IntoDiagnostic as _, bail};

/// Teardown for a prepared `simple` source, undone by [`teardown`].
#[derive(Debug)]
pub enum Cleanup {
	/// Snapshotted in place; nothing to release (non-Linux, where there's no
	/// kopia user and kopia runs as the current user).
	#[cfg(not(target_os = "linux"))]
	Nothing,
	/// A bindfs view to unmount and remove.
	#[cfg(target_os = "linux")]
	Bindfs(PathBuf),
	/// A copied tree to remove.
	#[cfg(target_os = "linux")]
	Copy(PathBuf),
}

/// Expose `source` as something the kopia user can read; returns the path kopia
/// should snapshot and the teardown to undo it.
pub async fn prepare(source: &Path, backup_type: &str) -> Result<(PathBuf, Cleanup)> {
	#[cfg(target_os = "linux")]
	{
		prepare_linux(source, backup_type).await
	}
	#[cfg(not(target_os = "linux"))]
	{
		let _ = backup_type;
		Ok((source.to_path_buf(), Cleanup::Nothing))
	}
}

/// Release whatever [`prepare`] set up.
pub async fn teardown(cleanup: Cleanup) -> Result<()> {
	match cleanup {
		#[cfg(not(target_os = "linux"))]
		Cleanup::Nothing => Ok(()),
		#[cfg(target_os = "linux")]
		Cleanup::Bindfs(view) => {
			unmount(&view).await;
			let _ = tokio::fs::remove_dir_all(&view).await;
			Ok(())
		}
		#[cfg(target_os = "linux")]
		Cleanup::Copy(dir) => tokio::fs::remove_dir_all(&dir)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("removing simple backup copy at {}", dir.display())),
	}
}

/// Where the kopia-readable view is exposed: under the daemon's CacheDirectory,
/// keyed by backup type so kopia's source identity is stable per type.
#[cfg(target_os = "linux")]
fn view_dir(backup_type: &str) -> PathBuf {
	dirs::cache_dir()
		.unwrap_or_else(|| PathBuf::from("/var/cache"))
		.join("bestool")
		.join("backup-source")
		.join(backup_type)
}

#[cfg(target_os = "linux")]
async fn prepare_linux(source: &Path, backup_type: &str) -> Result<(PathBuf, Cleanup)> {
	use bestool_kopia::LINUX_KOPIA_USER;
	use tokio::process::Command;

	let view = view_dir(backup_type);
	// Clear any leftover from a crashed run (a stale bindfs mount, or a copy).
	unmount(&view).await;
	if view.exists() {
		let _ = tokio::fs::remove_dir_all(&view).await;
	}
	tokio::fs::create_dir_all(&view)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("creating simple backup view dir {}", view.display()))?;

	// Preferred: a read-only bindfs view presenting the tree as kopia-owned.
	match Command::new("bindfs")
		.arg("-r")
		.arg(format!("--force-user={LINUX_KOPIA_USER}"))
		.arg(format!("--force-group={LINUX_KOPIA_USER}"))
		.arg("-o")
		.arg("allow_other")
		.arg(source)
		.arg(&view)
		.status()
		.await
	{
		Ok(status) if status.success() => {
			tracing::info!(source = %source.display(), view = %view.display(), "bindfs view for simple backup");
			return Ok((view.clone(), Cleanup::Bindfs(view)));
		}
		Ok(status) => {
			tracing::warn!(%status, "bindfs failed; copying the source instead");
			unmount(&view).await;
		}
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
			tracing::warn!("bindfs not installed; copying the source instead");
		}
		Err(err) => return Err(err).into_diagnostic().wrap_err("spawning bindfs"),
	}

	// Fallback: copy the tree and hand it to the kopia user.
	copy_to_kopia(source, &view).await?;
	Ok((view.clone(), Cleanup::Copy(view)))
}

/// `cp -a` the source contents into `dest`, then chown the lot to the kopia user.
#[cfg(target_os = "linux")]
async fn copy_to_kopia(source: &Path, dest: &Path) -> Result<()> {
	use bestool_kopia::LINUX_KOPIA_USER;
	use tokio::process::Command;

	let status = Command::new("cp")
		.arg("-a")
		.arg(format!("{}/.", source.display()))
		.arg(dest)
		.status()
		.await
		.into_diagnostic()
		.wrap_err("spawning cp for the simple backup copy")?;
	if !status.success() {
		bail!("copying simple backup source {} failed ({status})", source.display());
	}

	let status = Command::new("chown")
		.arg("-R")
		.arg(format!("{LINUX_KOPIA_USER}:{LINUX_KOPIA_USER}"))
		.arg(dest)
		.status()
		.await
		.into_diagnostic()
		.wrap_err("chowning the simple backup copy to the kopia user")?;
	if !status.success() {
		bail!("chowning the simple backup copy to {LINUX_KOPIA_USER} failed ({status})");
	}
	Ok(())
}

/// Best-effort unmount of a bindfs view (FUSE, so `fusermount -u`, else `umount`).
#[cfg(target_os = "linux")]
async fn unmount(view: &Path) {
	let unmounted = tokio::process::Command::new("fusermount")
		.arg("-u")
		.arg(view)
		.status()
		.await
		.map(|s| s.success())
		.unwrap_or(false);
	if !unmounted {
		let _ = tokio::process::Command::new("umount")
			.arg(view)
			.status()
			.await;
	}
}
