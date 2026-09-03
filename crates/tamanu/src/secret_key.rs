//! Where a host keeps the key that decrypts `local_system_secrets`.
//!
//! The key encrypts every value in that table: the settings PSK (and so every
//! secret setting), the device key, and a facility's sync password. A database
//! restored onto a host holding a different key reads none of them, so the key
//! has to be captured with the database it belongs to.
//!
//! Where it lives depends on how the server is installed, and the two shapes
//! are not both files: a bare-metal or Windows install points `crypto.keyFile`
//! at one, while a containerised install holds it as a podman secret, mounted
//! into the containers at the `/run/secrets` path its `crypto.keyFile` names.
//! Either way the key is a single value, readable and writable in both shapes.

use std::path::{Path, PathBuf};

use miette::{Result, bail, miette};

use crate::{ApiServerKind, config::load_config, detect_kind};

/// The config default when `crypto.keyFile` is unset, from
/// `packages/{central,facility}-server/config/default.json5` in Tamanu.
pub const DEFAULT_KEY_FILE: &str = "config/dev-secret.key";

/// Where podman mounts secrets inside a container. A `crypto.keyFile` under
/// here names a podman secret, not a host path.
pub const PODMAN_SECRETS_MOUNT: &str = "/run/secrets";

/// How a host holds the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretKeyLocation {
	/// A key file on the server's filesystem, at `crypto.keyFile`.
	KeyFile(PathBuf),
	/// A podman secret, named. Only its containers see it as a file; the host
	/// reads and writes the value through podman.
	PodmanSecret(String),
}

impl SecretKeyLocation {
	/// The shape's name, used in diagnostics.
	pub fn shape(&self) -> &'static str {
		match self {
			Self::KeyFile(_) => "key file",
			Self::PodmanSecret(_) => "podman secret",
		}
	}
}

impl std::fmt::Display for SecretKeyLocation {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::KeyFile(path) => write!(f, "{}", path.display()),
			Self::PodmanSecret(name) => write!(f, "podman secret {name}"),
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
	choose(resolve_key_file(key_file, root, kind)?)
}

/// Classify a resolved `crypto.keyFile` path.
///
/// A file that is there is the key. One under [`PODMAN_SECRETS_MOUNT`] is a
/// containerised install's view of a podman secret: the path only exists
/// inside the containers, and its basename names the secret the host holds.
/// Anything else is an error rather than a guess — a bare-metal host whose
/// named key file is missing has a real problem, and capturing something
/// other than its key would hide it.
fn choose(key_file: PathBuf) -> Result<SecretKeyLocation> {
	if key_file.is_file() {
		return Ok(SecretKeyLocation::KeyFile(key_file));
	}
	if let Ok(name) = key_file.strip_prefix(PODMAN_SECRETS_MOUNT) {
		let name = name
			.to_str()
			.filter(|name| !name.is_empty() && !name.contains('/'))
			.ok_or_else(|| {
				miette!(
					"crypto.keyFile ({}) does not name a podman secret",
					key_file.display()
				)
			})?;
		return Ok(SecretKeyLocation::PodmanSecret(name.into()));
	}
	bail!(
		"no key found: {} does not exist and does not name a podman secret",
		key_file.display()
	)
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

/// Whether a podman secret with this name exists on the host.
#[cfg(target_os = "linux")]
pub async fn podman_secret_exists(name: &str) -> Result<bool> {
	let output = run_podman(&["secret", "exists", name]).await?;
	match output.status.code() {
		Some(0) => Ok(true),
		Some(1) => Ok(false),
		_ => bail!(
			"podman secret exists {name} failed: {}",
			String::from_utf8_lossy(&output.stderr).trim()
		),
	}
}

/// Read a podman secret's value, byte-exact.
///
/// The template form is the one podman output that does not JSON-encode the
/// value, which would mangle a binary key. The report writer appends exactly
/// one newline to the template output, so popping one (and nothing else)
/// recovers the value even when it genuinely ends with a newline itself.
#[cfg(target_os = "linux")]
pub async fn read_podman_secret(name: &str) -> Result<Vec<u8>> {
	let output = run_podman(&[
		"secret",
		"inspect",
		"--showsecret",
		"--format",
		"{{.SecretData}}",
		name,
	])
	.await?;
	if !output.status.success() {
		bail!(
			"podman secret inspect {name} failed: {}",
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	let mut data = output.stdout;
	if data.last() == Some(&b'\n') {
		data.pop();
	}
	if data.is_empty() {
		bail!("podman secret {name} holds no data");
	}
	Ok(data)
}

/// Create or replace a podman secret with this value.
///
/// Containers resolve secrets by name when they are created, so quadlet
/// services pick the new value up on their next start.
#[cfg(target_os = "linux")]
pub async fn write_podman_secret(name: &str, value: &[u8]) -> Result<()> {
	use std::process::Stdio;

	use miette::{Context as _, IntoDiagnostic as _};
	use tokio::io::AsyncWriteExt as _;

	let mut child = crate::versions::podman_command()
		.args(["secret", "create", "--replace", name, "-"])
		.stdin(Stdio::piped())
		.stdout(Stdio::null())
		.stderr(Stdio::piped())
		.spawn()
		.into_diagnostic()
		.wrap_err("running podman secret create")?;
	let mut stdin = child
		.stdin
		.take()
		.ok_or_else(|| miette!("podman secret create has no stdin"))?;
	stdin.write_all(value).await.into_diagnostic()?;
	drop(stdin);
	let output = child
		.wait_with_output()
		.await
		.into_diagnostic()
		.wrap_err("waiting for podman secret create")?;
	if !output.status.success() {
		bail!(
			"podman secret create {name} failed: {}",
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	Ok(())
}

#[cfg(target_os = "linux")]
async fn run_podman(args: &[&str]) -> Result<std::process::Output> {
	use miette::{Context as _, IntoDiagnostic as _};

	crate::versions::podman_command()
		.args(args)
		.output()
		.await
		.into_diagnostic()
		.wrap_err("running podman")
}

/// A podman secret is a Linux shape; elsewhere these can only fail.
#[cfg(not(target_os = "linux"))]
pub async fn podman_secret_exists(name: &str) -> Result<bool> {
	bail!("podman secret {name} cannot exist here: podman secrets are a Linux shape")
}

#[cfg(not(target_os = "linux"))]
pub async fn read_podman_secret(name: &str) -> Result<Vec<u8>> {
	bail!("podman secret {name} cannot be read here: podman secrets are a Linux shape")
}

#[cfg(not(target_os = "linux"))]
pub async fn write_podman_secret(name: &str, _value: &[u8]) -> Result<()> {
	bail!("podman secret {name} cannot be written here: podman secrets are a Linux shape")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn absolute_key_file_passes_through() {
		let key = if cfg!(windows) {
			r"C:\Tamanu\config\tamanu.key"
		} else {
			"/etc/tamanu/tamanu.key"
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
	fn an_existing_key_file_is_the_key() {
		let tmp = tempfile::tempdir().unwrap();
		let key = tmp.path().join("tamanu.key");
		std::fs::write(&key, "k").unwrap();
		assert_eq!(
			choose(key.clone()).unwrap(),
			SecretKeyLocation::KeyFile(key)
		);
	}

	#[cfg(unix)]
	#[test]
	fn a_run_secrets_path_names_a_podman_secret() {
		assert_eq!(
			choose(PathBuf::from("/run/secrets/tamanu-config-key")).unwrap(),
			SecretKeyLocation::PodmanSecret("tamanu-config-key".into())
		);
	}

	#[cfg(unix)]
	#[test]
	fn a_nested_run_secrets_path_is_not_a_secret_name() {
		assert!(choose(PathBuf::from("/run/secrets/a/b")).is_err());
		assert!(choose(PathBuf::from("/run/secrets")).is_err());
	}

	#[test]
	fn a_missing_key_file_elsewhere_is_an_error() {
		let tmp = tempfile::tempdir().unwrap();
		let err = format!("{}", choose(tmp.path().join("absent.key")).unwrap_err());
		assert!(err.contains("absent.key"), "{err}");
	}
}
