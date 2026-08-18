//! Source preparation and restore for the `tamanu_secret_key` method.
//!
//! The key that decrypts `local_system_secrets` is held differently depending
//! on how the server is installed: a bare-metal or Windows install points
//! `crypto.keyFile` at a file, a containerised one takes it as a podman secret
//! and has no server-side path. The method resolves which, so a definition does
//! not have to name a platform.
//!
//! What lands in the repository is normalised: a directory holding either
//! `KEY_FILE_ENTRY` (the one file) or `PODMAN_STORE_ENTRY` (the store tree).
//! The entry name is the only record of which shape was captured, so a restore
//! can tell what it is holding without a manifest to version.

use std::path::{Path, PathBuf};

use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use tracing::{debug, info};

use bestool_tamanu::{
	find_tamanu,
	secret_key::{SecretKeyLocation, locate},
};

/// The captured key file, in the normalised tree.
const KEY_FILE_ENTRY: &str = "config-key";

/// The captured podman secret store, in the normalised tree.
const PODMAN_STORE_ENTRY: &str = "podman-secrets";

/// Find the key on this host.
pub(super) async fn location(config: &super::method::TamanuSecretKeyConfig) -> Result<SecretKeyLocation> {
	if let Some(path) = &config.path {
		return Ok(classify_target(path));
	}
	let (_, root) = find_tamanu(config.root.as_deref()).await?;
	locate(&root, config.package.as_deref()).await
}

/// An operator-given path says where, not which shape; a directory is the store,
/// a file (or one not written yet) is a key file.
pub(super) fn classify_target(path: &Path) -> SecretKeyLocation {
	if path.is_dir() {
		SecretKeyLocation::PodmanSecrets(path.to_path_buf())
	} else {
		SecretKeyLocation::KeyFile(path.to_path_buf())
	}
}

/// Build the normalised tree for `location` under `parent` and return it.
///
/// The tree is a copy, so it is a point in time: the key is a few hundred bytes
/// and the secret store little more, which is what makes copying it up front
/// cheaper than holding a consistent view of it for the length of a snapshot.
pub(super) async fn stage(
	location: &SecretKeyLocation,
	backup_type: &str,
	parent: &Path,
) -> Result<PathBuf> {
	let staged = parent.join(format!("secret-key.{backup_type}"));
	if staged.exists() {
		tokio::fs::remove_dir_all(&staged).await.ok();
	}
	tokio::fs::create_dir_all(&staged)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("creating {}", staged.display()))?;

	match location {
		SecretKeyLocation::KeyFile(key) => {
			let into = staged.join(KEY_FILE_ENTRY);
			tokio::fs::copy(key, &into)
				.await
				.into_diagnostic()
				.wrap_err_with(|| format!("copying {} to {}", key.display(), into.display()))?;
		}
		SecretKeyLocation::PodmanSecrets(store) => {
			copy_tree(store, &staged.join(PODMAN_STORE_ENTRY)).await?;
		}
	}
	debug!(location = ?location, staged = %staged.display(), "staged the tamanu secret key");
	Ok(staged)
}

/// Where the normalised tree is built before kopia reads it: the daemon's
/// CacheDirectory, the same place the simple method exposes its view, so root
/// can create it and the kopia user can still reach inside it.
#[cfg(target_os = "linux")]
pub(super) fn stage_parent() -> PathBuf {
	dirs::cache_dir()
		.unwrap_or_else(|| PathBuf::from("/var/cache"))
		.join("bestool")
		.join("backup-staging")
}

#[cfg(not(target_os = "linux"))]
pub(super) fn stage_parent() -> PathBuf {
	std::env::temp_dir()
}

/// Lay a restored tree back down where this host wants it.
///
/// Restoring a shape onto a host that wants the other is refused rather than
/// guessed at: converting a Windows key file into a podman secret (or either
/// into whatever Seedling ends up holding) is a real transformation, and a
/// half-right one leaves a server that starts and reads none of its secrets.
pub(super) async fn lay_down(
	staging: &Path,
	location: &SecretKeyLocation,
	clobber: bool,
) -> Result<()> {
	let captured = captured_shape(staging)?;
	match (&captured, location) {
		(Captured::KeyFile(from), SecretKeyLocation::KeyFile(to)) => {
			place_file(from, to, clobber).await
		}
		(Captured::PodmanSecrets(from), SecretKeyLocation::PodmanSecrets(to)) => {
			super::method::ensure_not_clobbering(to, clobber)?;
			super::method::replace_dir(from, to).await
		}
		_ => bail!(
			"the capture holds a {}, but this host keeps its key as a {} at {}; \
			 converting between the two is not implemented, so restore the key by hand",
			captured.shape(),
			location.shape(),
			location.path().display()
		),
	}
}

/// Which shape a restored tree holds.
enum Captured {
	KeyFile(PathBuf),
	PodmanSecrets(PathBuf),
}

impl Captured {
	fn shape(&self) -> &'static str {
		match self {
			Self::KeyFile(_) => "key file",
			Self::PodmanSecrets(_) => "podman secret store",
		}
	}
}

/// Read the shape out of a restored tree by which entry it carries.
fn captured_shape(staging: &Path) -> Result<Captured> {
	let key = staging.join(KEY_FILE_ENTRY);
	if key.is_file() {
		return Ok(Captured::KeyFile(key));
	}
	let store = staging.join(PODMAN_STORE_ENTRY);
	if store.is_dir() {
		return Ok(Captured::PodmanSecrets(store));
	}
	bail!(
		"the restored tree at {} holds neither {KEY_FILE_ENTRY} nor {PODMAN_STORE_ENTRY}, \
		 so it is not a tamanu_secret_key capture",
		staging.display()
	)
}

/// Write a single restored key file into place, keeping any existing one as
/// `<name>.old`. Same-directory rename, so the swap is atomic.
async fn place_file(from: &Path, to: &Path, clobber: bool) -> Result<()> {
	if to.exists() && !clobber {
		bail!(
			"{} already holds a key; refusing to overwrite without confirmation \
			 (pass --clobber-existing-data-yes-i-am-sure, or confirm interactively)",
			to.display()
		);
	}
	if let Some(parent) = to.parent() {
		tokio::fs::create_dir_all(parent).await.ok();
	}
	if to.exists() {
		let aside = super::method::with_extension_suffix(to, "old");
		tokio::fs::rename(to, &aside)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("moving {} aside to {}", to.display(), aside.display()))?;
	}
	tokio::fs::rename(from, to)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("moving the restored key into {}", to.display()))?;
	info!(key = %to.display(), "restored the tamanu config key");
	Ok(())
}

/// Copy a tree, preserving ownership and modes: podman reads its store back and
/// a secret with the wrong mode is a secret the server cannot use.
async fn copy_tree(source: &Path, into: &Path) -> Result<()> {
	#[cfg(unix)]
	{
		tokio::fs::create_dir_all(into)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("creating {}", into.display()))?;

		let mut from = source.as_os_str().to_owned();
		from.push("/.");
		let status = tokio::process::Command::new("cp")
			.arg("-a")
			.arg(&from)
			.arg(into)
			.status()
			.await
			.into_diagnostic()
			.wrap_err("running cp -a")?;
		if !status.success() {
			bail!("copying {} into {} failed: {status}", source.display(), into.display());
		}
		Ok(())
	}
	#[cfg(not(unix))]
	{
		let _ = into;
		bail!(
			"a podman secret store ({}) is a Linux shape and cannot be captured here",
			source.display()
		)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_target_is_classified_by_what_is_there() {
		let tmp = tempfile::tempdir().unwrap();
		let dir = tmp.path().join("secrets");
		std::fs::create_dir_all(&dir).unwrap();
		let file = tmp.path().join("tamanu.key");
		std::fs::write(&file, "k").unwrap();

		assert!(matches!(
			classify_target(&dir),
			SecretKeyLocation::PodmanSecrets(_)
		));
		assert!(matches!(
			classify_target(&file),
			SecretKeyLocation::KeyFile(_)
		));
		// An absent path is a key file that isn't written yet, not a store.
		assert!(matches!(
			classify_target(&tmp.path().join("absent")),
			SecretKeyLocation::KeyFile(_)
		));
	}

	#[test]
	fn the_captured_shape_is_read_from_the_entry_name() {
		let tmp = tempfile::tempdir().unwrap();
		let key_capture = tmp.path().join("as-file");
		std::fs::create_dir_all(&key_capture).unwrap();
		std::fs::write(key_capture.join(KEY_FILE_ENTRY), "k").unwrap();
		assert!(matches!(
			captured_shape(&key_capture).unwrap(),
			Captured::KeyFile(_)
		));

		let store_capture = tmp.path().join("as-store");
		std::fs::create_dir_all(store_capture.join(PODMAN_STORE_ENTRY)).unwrap();
		assert!(matches!(
			captured_shape(&store_capture).unwrap(),
			Captured::PodmanSecrets(_)
		));

		let neither = tmp.path().join("empty");
		std::fs::create_dir_all(&neither).unwrap();
		assert!(captured_shape(&neither).is_err());
	}

	#[tokio::test]
	async fn a_key_file_capture_is_the_one_file_under_its_entry_name() {
		let tmp = tempfile::tempdir().unwrap();
		let key = tmp.path().join("tamanu.key");
		std::fs::write(&key, "secret").unwrap();

		let staged = stage(
			&SecretKeyLocation::KeyFile(key),
			"test-key-file",
			tmp.path(),
		)
		.await
		.unwrap();
		assert_eq!(
			std::fs::read_to_string(staged.join(KEY_FILE_ENTRY)).unwrap(),
			"secret"
		);
	}

	#[tokio::test]
	async fn a_restored_key_keeps_the_displaced_one_beside_it() {
		let tmp = tempfile::tempdir().unwrap();
		let staging = tmp.path().join("staging");
		std::fs::create_dir_all(&staging).unwrap();
		std::fs::write(staging.join(KEY_FILE_ENTRY), "new").unwrap();
		let target = tmp.path().join("config").join("tamanu.key");
		std::fs::create_dir_all(target.parent().unwrap()).unwrap();
		std::fs::write(&target, "old").unwrap();

		let location = SecretKeyLocation::KeyFile(target.clone());
		// Occupied and not forced: refused before anything moves.
		assert!(lay_down(&staging, &location, false).await.is_err());
		assert_eq!(std::fs::read_to_string(&target).unwrap(), "old");

		lay_down(&staging, &location, true).await.unwrap();
		assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
		assert_eq!(
			std::fs::read_to_string(super::super::method::with_extension_suffix(&target, "old"))
				.unwrap(),
			"old"
		);
	}

	#[tokio::test]
	async fn a_shape_mismatch_is_refused_rather_than_guessed() {
		let tmp = tempfile::tempdir().unwrap();
		let staging = tmp.path().join("staging");
		std::fs::create_dir_all(&staging).unwrap();
		std::fs::write(staging.join(KEY_FILE_ENTRY), "k").unwrap();

		let err = lay_down(
			&staging,
			&SecretKeyLocation::PodmanSecrets(tmp.path().join("secrets")),
			true,
		)
		.await
		.unwrap_err();
		let err = format!("{err}");
		assert!(err.contains("not implemented"), "{err}");
		assert!(err.contains("podman secret store"), "{err}");
	}
}
