//! Shared system-command and mount helpers for the snapshot strategies
//! (btrfs, thin-LVM). Thin wrappers around `findmnt`, `mount`, `id`, etc.; the
//! privileged calls are verified on-host, the pure bits (idmap, path math) are
//! unit-tested.

use std::{
	path::{Path, PathBuf},
	process::Stdio,
};

use miette::{Context as _, IntoDiagnostic as _, Result, bail, miette};

/// A `Path` as a `&str` (lossy-empty if non-UTF-8 — our paths are ASCII).
pub(super) fn path(p: &Path) -> &str {
	p.to_str().unwrap_or_default()
}

/// Run a command, erroring (with stderr) on non-zero exit.
pub(super) async fn run_ok(program: &str, args: &[&str]) -> Result<()> {
	let output = tokio::process::Command::new(program)
		.args(args)
		.stdin(Stdio::null())
		.output()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("spawning {program}"))?;
	if !output.status.success() {
		bail!(
			"{program} {} failed: {}",
			args.join(" "),
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	Ok(())
}

/// Run a command and return its trimmed stdout, erroring on non-zero exit.
pub(super) async fn capture(program: &str, args: &[&str]) -> Result<String> {
	let output = tokio::process::Command::new(program)
		.args(args)
		.stdin(Stdio::null())
		.output()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("spawning {program}"))?;
	if !output.status.success() {
		bail!(
			"{program} {} failed: {}",
			args.join(" "),
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The mountpoint of the filesystem backing `data_dir`.
pub(super) async fn findmnt_target(data_dir: &Path) -> Result<PathBuf> {
	Ok(PathBuf::from(findmnt_field("TARGET", data_dir).await?))
}

/// A single `findmnt` field (`UUID`, `SOURCE`, `FSTYPE`, …) for `data_dir`.
pub(super) async fn findmnt_field(field: &str, data_dir: &Path) -> Result<String> {
	capture("findmnt", &["-no", field, "--target", path(data_dir)]).await
}

/// A user's numeric uid / gid (via `id`).
pub(super) async fn uid_of(user: &str) -> Result<u32> {
	parse_id(&capture("id", &["-u", user]).await?, user)
}

pub(super) async fn gid_of(user: &str) -> Result<u32> {
	parse_id(&capture("id", &["-g", user]).await?, user)
}

fn parse_id(out: &str, user: &str) -> Result<u32> {
	out.trim()
		.parse()
		.into_diagnostic()
		.wrap_err_with(|| format!("parsing id for {user}: {out:?}"))
}

pub(super) async fn mkdir(dir: &Path) -> Result<()> {
	tokio::fs::create_dir_all(dir)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("creating {}", dir.display()))
}

/// Make `dir` world-traversable (mode 0o755) so the unprivileged kopia user can
/// descend through it to the mounted source. The daemon runs with a restrictive
/// umask, which would otherwise leave a freshly-created `backup-source` dir
/// group-only and deny the kopia user. The dir is a bare mountpoint parent — the
/// mounted snapshot keeps its own (idmapped) permissions.
pub(super) async fn make_traversable(dir: &Path) -> Result<()> {
	#[cfg(unix)]
	{
		use std::os::unix::fs::PermissionsExt as _;
		tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("making {} traversable", dir.display()))?;
	}
	#[cfg(not(unix))]
	let _ = dir;
	Ok(())
}

pub(super) async fn umount(dir: &Path) {
	if is_mountpoint(dir).await {
		let _ = run_ok("umount", &[path(dir)]).await;
	}
}

pub(super) async fn rmdir(dir: &Path) {
	let _ = tokio::fs::remove_dir(dir).await;
}

pub(super) async fn is_mountpoint(dir: &Path) -> bool {
	tokio::process::Command::new("mountpoint")
		.arg("-q")
		.arg(dir)
		.stdin(Stdio::null())
		.status()
		.await
		.map(|s| s.success())
		.unwrap_or(false)
}

/// Directory entries whose file name starts with `prefix`.
pub(super) fn glob_prefix(dir: impl AsRef<Path>, prefix: &str) -> Result<Vec<PathBuf>> {
	let mut out = Vec::new();
	for entry in std::fs::read_dir(dir.as_ref()).into_diagnostic()? {
		let entry = entry.into_diagnostic()?;
		if entry.file_name().to_string_lossy().starts_with(prefix) {
			out.push(entry.path());
		}
	}
	Ok(out)
}

/// The `X-mount.idmap` mapping postgres's uid/gid to kopia's, so the kopia user
/// can read the postgres-owned files in a read-only snapshot mount.
pub(super) fn idmap(postgres_uid: u32, kopia_uid: u32, postgres_gid: u32, kopia_gid: u32) -> String {
	format!("u:{postgres_uid}:{kopia_uid}:1 g:{postgres_gid}:{kopia_gid}:1")
}

/// Build the postgres→kopia idmap by resolving both users' ids.
pub(super) async fn postgres_to_kopia_idmap() -> Result<String> {
	Ok(idmap(
		uid_of("postgres").await?,
		uid_of("kopia").await?,
		gid_of("postgres").await?,
		gid_of("kopia").await?,
	))
}

/// The cluster directory's path relative to its filesystem mountpoint.
pub(super) fn relative_data_path(data_dir: &Path, base_mount: &Path) -> Result<PathBuf> {
	data_dir
		.strip_prefix(base_mount)
		.map(Path::to_path_buf)
		.map_err(|_| {
			miette!(
				"data dir {} is not under its mountpoint {}",
				data_dir.display(),
				base_mount.display()
			)
		})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn idmap_format() {
		assert_eq!(idmap(114, 997, 120, 995), "u:114:997:1 g:120:995:1");
	}

	#[test]
	fn relative_data_path_strips_mountpoint() {
		let rel = relative_data_path(
			Path::new("/var/lib/postgresql/16/main"),
			Path::new("/var/lib/postgresql"),
		)
		.unwrap();
		assert_eq!(rel, PathBuf::from("16/main"));
	}

	#[test]
	fn relative_data_path_rejects_outside() {
		assert!(relative_data_path(Path::new("/srv/pg"), Path::new("/var/lib/postgresql")).is_err());
	}
}
