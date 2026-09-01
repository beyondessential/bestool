//! Source preparation and restore for the `tamanu_secret_key` method.
//!
//! The key that decrypts `local_system_secrets` is held differently depending
//! on how the server is installed: a bare-metal or Windows install points
//! `crypto.keyFile` at a file, a containerised one holds it as a podman secret.
//! The method resolves which, so a definition does not have to name a platform.
//!
//! What lands in the repository is the key value itself: a directory holding
//! the one file `KEY_FILE_ENTRY`, whatever shape the host held it in. The value
//! is all the database needs, which is what lets a capture taken from one shape
//! restore onto the other.

use std::path::{Path, PathBuf};

use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use tracing::{debug, info};

use bestool_tamanu::{
	find_tamanu,
	secret_key::{
		SecretKeyLocation, locate, podman_secret_exists, read_podman_secret, write_podman_secret,
	},
};

/// The captured key, in the staged tree.
const KEY_FILE_ENTRY: &str = "config-key";

/// Find the key on this host.
pub(super) async fn location(
	config: &super::method::TamanuSecretKeyConfig,
) -> Result<SecretKeyLocation> {
	if let Some(path) = &config.path {
		return classify_target(path);
	}
	let (_, root) = find_tamanu(config.root.as_deref()).await?;
	locate(&root, config.package.as_deref()).await
}

/// An operator-given path names the key file to read or write.
pub(super) fn classify_target(path: &Path) -> Result<SecretKeyLocation> {
	if path.is_dir() {
		bail!(
			"{} is a directory; the capture is a single key value, so name the key file itself",
			path.display()
		);
	}
	Ok(SecretKeyLocation::KeyFile(path.to_path_buf()))
}

/// Build the staged tree for `location` under `parent` and return it.
///
/// The tree is a copy, so it is a point in time: the key is a few hundred
/// bytes, which is what makes copying it up front cheaper than holding a
/// consistent view of it for the length of a snapshot.
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

	let into = staged.join(KEY_FILE_ENTRY);
	match location {
		SecretKeyLocation::KeyFile(key) => {
			tokio::fs::copy(key, &into)
				.await
				.into_diagnostic()
				.wrap_err_with(|| format!("copying {} to {}", key.display(), into.display()))?;
		}
		SecretKeyLocation::PodmanSecret(name) => {
			let value = read_podman_secret(name).await?;
			tokio::fs::write(&into, &value)
				.await
				.into_diagnostic()
				.wrap_err_with(|| format!("writing {}", into.display()))?;
			#[cfg(unix)]
			{
				use std::os::unix::fs::PermissionsExt as _;
				tokio::fs::set_permissions(&into, std::fs::Permissions::from_mode(0o600))
					.await
					.into_diagnostic()?;
			}
		}
	}
	debug!(location = %location, staged = %staged.display(), "staged the tamanu secret key");
	Ok(staged)
}

/// Where the staged tree is built before kopia reads it: the daemon's
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

/// Write a restored key value back in whatever shape this host keeps its key.
///
/// The capture is the value, so either shape can receive it: a key file is
/// swapped in by rename, a podman secret is recreated through podman. Either
/// way the key it displaces is kept beside it, as `<name>.old`.
pub(super) async fn lay_down(
	staging: &Path,
	location: &SecretKeyLocation,
	clobber: bool,
) -> Result<()> {
	let captured = staged_key(staging)?;
	match location {
		SecretKeyLocation::KeyFile(to) => place_file(&captured, to, clobber).await,
		SecretKeyLocation::PodmanSecret(name) => place_secret(&captured, name, clobber).await,
	}
}

/// The captured key inside a restored tree.
fn staged_key(staging: &Path) -> Result<PathBuf> {
	let key = staging.join(KEY_FILE_ENTRY);
	if key.is_file() {
		return Ok(key);
	}
	bail!(
		"the restored tree at {} does not hold {KEY_FILE_ENTRY}, \
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

/// Recreate a podman secret from a restored key, keeping any existing value as
/// a `<name>.old` secret.
async fn place_secret(from: &Path, name: &str, clobber: bool) -> Result<()> {
	let value = tokio::fs::read(from)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("reading the restored key at {}", from.display()))?;
	if podman_secret_exists(name).await? {
		if !clobber {
			bail!(
				"podman secret {name} already holds a key; refusing to overwrite without \
				 confirmation (pass --clobber-existing-data-yes-i-am-sure, or confirm \
				 interactively)"
			);
		}
		let displaced = read_podman_secret(name).await?;
		write_podman_secret(&format!("{name}.old"), &displaced).await?;
	}
	write_podman_secret(name, &value).await?;
	info!(secret = name, "restored the tamanu config key");
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A real podman round trip: every containerised install keeps the key as a
	/// secret, and nothing else exercises that shape against podman itself.
	///
	/// spec: BAK#the-tamanu_secret_key-method
	#[cfg(target_os = "linux")]
	#[tokio::test]
	#[ignore = "needs podman and root; run on a containerised host"]
	async fn a_podman_secret_round_trips_through_a_capture() {
		struct Guard(Vec<String>);
		impl Drop for Guard {
			fn drop(&mut self) {
				for secret in &self.0 {
					let _ = std::process::Command::new("podman")
						.args(["secret", "rm", secret])
						.status();
				}
			}
		}

		let name = format!("bestool-test-key-{}", std::process::id());
		let displaced = format!("{name}.old");
		let _guard = Guard(vec![name.clone(), displaced.clone()]);

		write_podman_secret(&name, b"captured-key")
			.await
			.expect("create the scratch secret");

		let parent = tempfile::tempdir().expect("staging parent");
		let location = SecretKeyLocation::PodmanSecret(name.clone());
		let staged = stage(&location, "test-secret-key", parent.path())
			.await
			.expect("stage the podman secret");

		let entry = staged.join(KEY_FILE_ENTRY);
		assert_eq!(
			std::fs::read(&entry).expect("read the staged key"),
			b"captured-key",
			"the staged tree holds the secret's value"
		);
		{
			use std::os::unix::fs::PermissionsExt as _;
			let mode = std::fs::metadata(&entry).unwrap().permissions().mode();
			assert_eq!(mode & 0o777, 0o600, "a staged key is readable by root alone");
		}

		// The host's key moves on after the capture: a restore puts the captured
		// one back and keeps what it displaced.
		write_podman_secret(&name, b"live-key")
			.await
			.expect("replace the live secret");
		lay_down(&staged, &location, true)
			.await
			.expect("restore the captured key");

		assert_eq!(
			read_podman_secret(&name).await.expect("read the restored secret"),
			b"captured-key"
		);
		assert_eq!(
			read_podman_secret(&displaced)
				.await
				.expect("read the displaced secret"),
			b"live-key",
			"the key the restore displaced is kept beside it"
		);
	}

	#[test]
	fn a_target_names_a_key_file_never_a_directory() {
		let tmp = tempfile::tempdir().unwrap();
		let dir = tmp.path().join("secrets");
		std::fs::create_dir_all(&dir).unwrap();
		let file = tmp.path().join("tamanu.key");
		std::fs::write(&file, "k").unwrap();

		assert!(classify_target(&dir).is_err());
		assert!(matches!(
			classify_target(&file).unwrap(),
			SecretKeyLocation::KeyFile(_)
		));
		// An absent path is a key file that isn't written yet.
		assert!(matches!(
			classify_target(&tmp.path().join("absent")).unwrap(),
			SecretKeyLocation::KeyFile(_)
		));
	}

	#[test]
	fn a_restored_tree_must_hold_the_key_entry() {
		let tmp = tempfile::tempdir().unwrap();
		let capture = tmp.path().join("capture");
		std::fs::create_dir_all(&capture).unwrap();
		std::fs::write(capture.join(KEY_FILE_ENTRY), "k").unwrap();
		assert!(staged_key(&capture).is_ok());

		let empty = tmp.path().join("empty");
		std::fs::create_dir_all(&empty).unwrap();
		assert!(staged_key(&empty).is_err());
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
}
