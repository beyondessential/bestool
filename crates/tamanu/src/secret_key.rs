//! Where a host keeps the key that decrypts `local_system_secrets`.
//!
//! The key encrypts every value in that table: the settings PSK (and so every
//! secret setting), the device key, and a facility's sync password. A database
//! restored onto a host holding a different key reads none of them, so the key
//! has to be captured with the database it belongs to.
//!
//! Where it lives depends on how the server is installed, and the two shapes
//! are not both files: a bare-metal or Windows install points `crypto.keyFile`
//! at one, while a containerised install takes it as a podman secret and has no
//! server-side path at all.

use std::path::{Path, PathBuf};

use miette::{Result, bail};

use crate::{ApiServerKind, config::load_config, detect_kind};

/// The config default when `crypto.keyFile` is unset, from
/// `packages/{central,facility}-server/config/default.json5` in Tamanu.
pub const DEFAULT_KEY_FILE: &str = "config/dev-secret.key";

/// Podman's rootful secret store. Tamanu's containers are root-owned, so the
/// rootless store under a user's home is not where the key lands.
pub const PODMAN_SECRET_STORE: &str = "/var/lib/containers/storage/secrets";

/// How a host holds the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretKeyLocation {
	/// A key file on the server's filesystem, at `crypto.keyFile`.
	KeyFile(PathBuf),
	/// Podman's secret store, which a containerised server takes the key from.
	/// The store holds every secret on the host, not only Tamanu's, and podman
	/// owns its layout: it is captured and laid back whole.
	PodmanSecrets(PathBuf),
}

impl SecretKeyLocation {
	/// The path to read or write, whichever shape this is.
	pub fn path(&self) -> &Path {
		match self {
			Self::KeyFile(path) | Self::PodmanSecrets(path) => path,
		}
	}

	/// The shape's name, used in diagnostics.
	pub fn shape(&self) -> &'static str {
		match self {
			Self::KeyFile(_) => "key file",
			Self::PodmanSecrets(_) => "podman secret store",
		}
	}
}

/// Find the key on this host, reading the server config for `crypto.keyFile`.
///
/// `package` picks which server's config to read; when `None` it is detected.
pub async fn locate(root: &Path, package: Option<&str>) -> Result<SecretKeyLocation> {
	let config = load_config(root, package)?;
	let kind = match package.and_then(ApiServerKind::from_str_ci) {
		Some(kind) => kind,
		None => detect_kind(&config, None).await,
	};
	let key_file = config
		.crypto
		.as_ref()
		.and_then(|crypto| crypto.key_file.as_deref())
		.unwrap_or(DEFAULT_KEY_FILE);
	choose(
		resolve_key_file(key_file, root, kind).ok().as_deref(),
		Path::new(PODMAN_SECRET_STORE),
	)
}

/// Pick between a resolved key file and podman's store.
///
/// A key file that is not there is not the answer even when the config names
/// one: a containerised install carries the server's own default in its config
/// while the key itself only exists as a podman secret. So the file has to
/// exist to win, and the store is the fallback rather than the other way round,
/// which keeps a bare-metal host with a genuinely missing key an error instead
/// of a silent capture of the wrong thing.
fn choose(key_file: Option<&Path>, store: &Path) -> Result<SecretKeyLocation> {
	if let Some(path) = key_file
		&& path.is_file()
	{
		return Ok(SecretKeyLocation::KeyFile(path.to_path_buf()));
	}
	if store.is_dir() {
		return Ok(SecretKeyLocation::PodmanSecrets(store.to_path_buf()));
	}
	match key_file {
		Some(path) => bail!(
			"no key found: {} does not exist and there is no podman secret store at {}",
			path.display(),
			store.display()
		),
		None => bail!(
			"no key found: crypto.keyFile could not be resolved and there is no \
			 podman secret store at {}",
			store.display()
		),
	}
}

/// Resolve a relative key file the way the server does: against its working
/// directory, the server package directory under the install root. An absolute
/// path passes through. A relative path with no package directory to resolve
/// against is an error rather than a guess.
pub fn resolve_key_file(key_file: &str, root: &Path, kind: ApiServerKind) -> Result<PathBuf> {
	let path = Path::new(key_file);
	if path.is_absolute() {
		return Ok(path.to_path_buf());
	}
	let base = root.join("packages").join(kind.package_name());
	if !base.is_dir() {
		bail!(
			"crypto.keyFile is relative ({key_file}) and there is no {} to resolve it against; \
			 set it to an absolute path",
			base.display()
		);
	}
	Ok(base.join(path))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn absolute_key_file_passes_through() {
		let key = if cfg!(windows) {
			r"C:\Tamanu\config\tamanu.key"
		} else {
			"/run/secrets/tamanu-config-key"
		};
		assert_eq!(
			resolve_key_file(key, Path::new("/nonexistent"), ApiServerKind::Central).unwrap(),
			PathBuf::from(key)
		);
	}

	#[test]
	fn relative_key_file_resolves_against_the_package_dir() {
		let tmp = tempfile::tempdir().unwrap();
		let package_dir = tmp.path().join("packages").join("facility-server");
		std::fs::create_dir_all(&package_dir).unwrap();
		assert_eq!(
			resolve_key_file(DEFAULT_KEY_FILE, tmp.path(), ApiServerKind::Facility).unwrap(),
			package_dir.join(DEFAULT_KEY_FILE)
		);
		assert!(resolve_key_file(DEFAULT_KEY_FILE, tmp.path(), ApiServerKind::Central).is_err());
	}

	#[test]
	fn an_existing_key_file_wins_over_the_store() {
		let tmp = tempfile::tempdir().unwrap();
		let key = tmp.path().join("tamanu.key");
		std::fs::write(&key, "k").unwrap();
		let store = tmp.path().join("secrets");
		std::fs::create_dir_all(&store).unwrap();
		assert_eq!(
			choose(Some(&key), &store).unwrap(),
			SecretKeyLocation::KeyFile(key)
		);
	}

	#[test]
	fn a_config_named_key_file_that_is_not_there_falls_back_to_the_store() {
		let tmp = tempfile::tempdir().unwrap();
		let store = tmp.path().join("secrets");
		std::fs::create_dir_all(&store).unwrap();
		assert_eq!(
			choose(Some(&tmp.path().join("absent.key")), &store).unwrap(),
			SecretKeyLocation::PodmanSecrets(store)
		);
	}

	#[test]
	fn neither_present_names_both_in_the_error() {
		let tmp = tempfile::tempdir().unwrap();
		let key = tmp.path().join("absent.key");
		let store = tmp.path().join("no-store");
		let err = format!("{}", choose(Some(&key), &store).unwrap_err());
		assert!(err.contains("absent.key"), "{err}");
		assert!(err.contains("no-store"), "{err}");
	}
}
