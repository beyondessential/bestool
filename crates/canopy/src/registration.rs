//! Single, machine-bound, encrypted store for this host's canopy enrollment.
//!
//! Everything the agent needs to talk to canopy — the mTLS device key, the
//! server id, and (once enrolled) the device id and api url — lives in one
//! encrypted file:
//!
//! - Linux: `/etc/bestool/canopy-registration`
//! - Windows: `%ProgramData%\bestool\canopy-registration`
//!
//! Encryption reuses algae (the age/scrypt profile this workspace already uses
//! for `protect`/`reveal` and the enrollment ticket). The local file is keyed
//! by a passphrase derived from the host's machine id, so a cloned disk can't
//! reuse it on a different machine and the device key isn't at rest in
//! plaintext. The same format is used for `canopy export` blobs, keyed by an
//! operator passphrase instead — see [`encrypt_with_passphrase`].
//!
//! The machine-id binding is a deliberately weak, software-only measure. Where
//! a TPM is present it could augment this — sealing or deriving the unlock key
//! in hardware via [`machine_passphrase`] — while hosts without one keep using
//! the machine id, and neither the file format nor any consumer changes.

use std::{
	fmt,
	path::{Path, PathBuf},
};

use algae_cli::passphrases::Passphrase;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::machine_store::{
	DIR_ENV, WORK_FACTOR, decrypt_bytes, encrypt_bytes, machine_passphrase, remove_if_present,
	scrypt_work_factor, write_atomic,
};

const VERSION: &str = "registration-1";

/// This host's canopy enrollment state.
///
/// Every field is optional so a partially-provisioned or migrated host can
/// still be represented; `canopy register` populates all of them.
#[derive(Clone, Serialize, Deserialize)]
pub struct Registration {
	pub v: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub server_id: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub device_key: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub device_id: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub api_url: Option<String>,
}

impl Default for Registration {
	fn default() -> Self {
		Self {
			v: VERSION.to_owned(),
			server_id: None,
			device_key: None,
			device_id: None,
			api_url: None,
		}
	}
}

impl fmt::Debug for Registration {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("Registration")
			.field("v", &self.v)
			.field("server_id", &self.server_id)
			.field(
				"device_key",
				&self.device_key.as_ref().map(|_| "<redacted>"),
			)
			.field("device_id", &self.device_id)
			.field("api_url", &self.api_url)
			.finish()
	}
}

/// Default base directory for the registration file.
///
/// Shared with every other machine-bound file this host keeps, so all per-host
/// canopy state lives in one place; see [`crate::machine_store::default_dir`].
pub fn default_dir() -> PathBuf {
	crate::machine_store::default_dir()
}

fn registration_file(dir: &Path) -> PathBuf {
	dir.join("canopy-registration")
}

/// Path to the cached canopy tags file, alongside the registration.
///
/// Tags aren't secret, so this is a plaintext JSON file rather than part of the
/// encrypted registration blob; it lives in the same directory ([`default_dir`],
/// honouring [`DIR_ENV`]) so all per-host canopy state shares one location.
pub fn default_tags_path() -> PathBuf {
	default_dir().join("tags.json")
}

// Legacy plaintext paths, mirroring bestool-tamanu's `standard_*` paths. Kept
// as literals here because canopy can't depend on the tamanu crate.
fn legacy_server_id_path() -> PathBuf {
	if cfg!(windows) {
		PathBuf::from(r"C:\Tamanu\server-id")
	} else {
		PathBuf::from("/etc/tamanu/server-id")
	}
}

fn legacy_device_key_path() -> PathBuf {
	if cfg!(windows) {
		PathBuf::from(r"C:\Tamanu\device-key.pem")
	} else {
		PathBuf::from("/etc/tamanu/device-key.pem")
	}
}

/// Process-wide cache of the registration at the default location, so repeated
/// reporting reads (e.g. the doctor tick) don't re-run scrypt each time. A
/// [`store`] at the default location refreshes it, so a writer's update — the
/// self-heal that recovers a missing identity, say — is seen by the next
/// in-process read without waiting for a process restart.
static CACHE: std::sync::RwLock<Option<Registration>> = std::sync::RwLock::new(None);

/// Load the registration from the default location.
///
/// If the file is absent, migrates from the legacy `/etc/tamanu` plaintext
/// files (unless [`DIR_ENV`] is set). Returns `None` when there's nothing to
/// load.
pub async fn load() -> Result<Option<Registration>> {
	if let Some(reg) = CACHE.read().expect("registration cache poisoned").as_ref() {
		return Ok(Some(reg.clone()));
	}

	let dir = default_dir();
	let path = registration_file(&dir);
	let reg = if path.exists() {
		Some(read_and_decrypt(&path).await?)
	} else if std::env::var_os(DIR_ENV).is_some() {
		None
	} else {
		migrate_from_legacy(&dir).await?
	};

	if let Some(ref reg) = reg {
		set_cache(reg.clone());
	}
	Ok(reg)
}

/// Replace the process-wide cache of the default-location registration.
fn set_cache(reg: Registration) {
	*CACHE.write().expect("registration cache poisoned") = Some(reg);
}

/// Load the registration from a specific directory, without legacy migration.
pub async fn load_from(dir: &Path) -> Result<Option<Registration>> {
	let path = registration_file(dir);
	if path.exists() {
		Ok(Some(read_and_decrypt(&path).await?))
	} else {
		Ok(None)
	}
}

/// Encrypt and store the registration at the default location.
///
/// Refreshes the process-wide [`CACHE`] on success so an in-process reader sees
/// the update on its next [`load`] without a restart.
pub async fn store(reg: &Registration) -> Result<()> {
	store_in(&default_dir(), reg).await?;
	set_cache(reg.clone());
	Ok(())
}

/// Encrypt and store the registration in a specific directory.
pub async fn store_in(dir: &Path, reg: &Registration) -> Result<()> {
	tokio::fs::create_dir_all(dir)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("creating {}", dir.display()))?;
	let plaintext = serde_json::to_vec(reg)
		.into_diagnostic()
		.wrap_err("serialising registration")?;
	let ciphertext = encrypt_bytes(&plaintext, machine_passphrase()?)?;
	write_atomic(&registration_file(dir), &ciphertext).await
}

/// Remove the registration file (and any stale temp file) from `dir`.
///
/// Returns whether a registration file was present. A running daemon caches the
/// registration in memory for its lifetime, so it must be restarted to notice
/// the removal.
pub async fn delete_in(dir: &Path) -> Result<bool> {
	let path = registration_file(dir);
	let existed = remove_if_present(&path).await?;
	// Best-effort: a leftover temp file isn't an enrollment, so a failure to
	// remove it shouldn't fail the unregister.
	let _ = remove_if_present(&path.with_extension("tmp")).await;
	Ok(existed)
}

/// Encrypt a registration under an operator passphrase, for `canopy export`.
pub fn encrypt_with_passphrase(reg: &Registration, passphrase: Passphrase) -> Result<Vec<u8>> {
	let plaintext = serde_json::to_vec(reg)
		.into_diagnostic()
		.wrap_err("serialising registration")?;
	encrypt_bytes(&plaintext, passphrase)
}

/// Generate a fresh random passphrase for `canopy export`.
///
/// ~128 bits from a URL-safe base64 of 16 random bytes — enough entropy to make
/// brute force infeasible, with no wordlist to bloat the binary.
pub fn generate_passphrase() -> Result<String> {
	let mut bytes = [0u8; 16];
	getrandom::fill(&mut bytes).map_err(|e| miette!("generating passphrase: {e}"))?;
	Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// Decrypt a registration from an operator passphrase, for `canopy import`.
pub fn decrypt_with_passphrase(bytes: &[u8], passphrase: Passphrase) -> Result<Registration> {
	let plaintext = decrypt_bytes(bytes, passphrase)?;
	serde_json::from_slice(&plaintext)
		.into_diagnostic()
		.wrap_err("parsing registration")
}

async fn read_and_decrypt(path: &Path) -> Result<Registration> {
	#[cfg(unix)]
	crate::machine_store::repair_mode(path).await;

	let bytes = tokio::fs::read(path)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("reading {}", path.display()))?;
	let plaintext = decrypt_bytes(&bytes, machine_passphrase()?)
		.wrap_err("decrypting registration (was this disk cloned from another machine?)")?;

	// Files written before the work factor was fixed used age's calibrated
	// default, which costs hundreds of MiB to decrypt on every load. Re-encrypt
	// once with the cheap factor. Best-effort: unprivileged readers can't write
	// here, and the owner will on its next load.
	if scrypt_work_factor(&bytes).is_some_and(|log_n| log_n > WORK_FACTOR) {
		match encrypt_bytes(&plaintext, machine_passphrase()?) {
			Ok(cheap) => match write_atomic(path, &cheap).await {
				Ok(()) => {
					info!(path = %path.display(), "re-encrypted registration with cheap work factor")
				}
				Err(err) => debug!(%err, "could not rewrite registration with cheap work factor"),
			},
			Err(err) => debug!(%err, "could not re-encrypt registration with cheap work factor"),
		}
	}

	serde_json::from_slice(&plaintext)
		.into_diagnostic()
		.wrap_err("parsing registration")
}

async fn migrate_from_legacy(dir: &Path) -> Result<Option<Registration>> {
	let sid_path = legacy_server_id_path();
	let key_path = legacy_device_key_path();
	let server_id = read_trimmed(&sid_path);
	let device_key = std::fs::read_to_string(&key_path)
		.ok()
		.filter(|s| !s.trim().is_empty());

	if server_id.is_none() && device_key.is_none() {
		return Ok(None);
	}

	let reg = Registration {
		server_id,
		device_key,
		..Registration::default()
	};
	info!("migrating canopy registration from legacy /etc/tamanu files");

	// Write the consolidated file, then prove it reads back from scratch before
	// removing the only other copy of the device key. Any failure leaves the
	// legacy files in place so the next run retries.
	if let Err(err) = store_in(dir, &reg).await {
		warn!(%err, "could not write consolidated registration; keeping legacy files");
		return Ok(Some(reg));
	}
	match load_from(dir).await {
		Ok(Some(roundtrip))
			if roundtrip.server_id == reg.server_id && roundtrip.device_key == reg.device_key =>
		{
			delete_legacy(&sid_path, &key_path);
		}
		Ok(_) => warn!("registration did not round-trip; keeping legacy files"),
		Err(err) => warn!(%err, "could not verify written registration; keeping legacy files"),
	}

	Ok(Some(reg))
}

fn delete_legacy(sid_path: &Path, key_path: &Path) {
	for path in [sid_path, key_path] {
		match std::fs::remove_file(path) {
			Ok(()) => debug!(path = %path.display(), "removed migrated legacy file"),
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
			Err(err) => warn!(path = %path.display(), %err, "could not remove legacy file"),
		}
	}
}

fn read_trimmed(path: &Path) -> Option<String> {
	std::fs::read_to_string(path)
		.ok()
		.map(|s| s.trim().to_owned())
		.filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
	#[cfg(unix)]
	use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

	use super::*;
	#[cfg(unix)]
	use crate::machine_store::FILE_MODE;
	use crate::machine_store::derive_passphrase;

	fn passphrase(s: &str) -> Passphrase {
		Passphrase::new(s.to_owned().into())
	}

	fn sample() -> Registration {
		Registration {
			server_id: Some("7deb2793-0425-427e-8a19-7213946fa9be".into()),
			device_key: Some(
				"-----BEGIN PRIVATE KEY-----\nMIG...\n-----END PRIVATE KEY-----\n".into(),
			),
			device_id: Some("11111111-2222-3333-4444-555555555555".into()),
			api_url: Some("https://canopy.example/".into()),
			..Registration::default()
		}
	}

	#[test]
	fn debug_redacts_device_key() {
		let dbg = format!("{:?}", sample());
		assert!(dbg.contains("<redacted>"), "{dbg}");
		assert!(!dbg.contains("BEGIN PRIVATE KEY"), "{dbg}");
	}

	#[test]
	fn passphrase_roundtrip() {
		let reg = sample();
		let blob = encrypt_with_passphrase(&reg, passphrase("a-test-passphrase")).unwrap();
		let back = decrypt_with_passphrase(&blob, passphrase("a-test-passphrase")).unwrap();
		assert_eq!(back.server_id, reg.server_id);
		assert_eq!(back.device_key, reg.device_key);
		assert_eq!(back.device_id, reg.device_id);
		assert_eq!(back.api_url, reg.api_url);
	}

	#[test]
	fn passphrase_decrypt_rejects_wrong_passphrase() {
		let blob = encrypt_with_passphrase(&sample(), passphrase("right-passphrase")).unwrap();
		assert!(decrypt_with_passphrase(&blob, passphrase("wrong-passphrase")).is_err());
	}

	#[tokio::test]
	async fn load_reads_and_set_cache_refreshes_the_process_cache() {
		// load() short-circuits on the process cache; set_cache (which store
		// calls) replaces it. A refreshed value must be returned rather than
		// frozen at the first read — that freeze would keep a healed
		// registration invisible until the daemon restarted.
		let first = Registration {
			server_id: Some("s".into()),
			..Registration::default()
		};
		set_cache(first.clone());
		assert_eq!(load().await.unwrap().unwrap().device_id, None);

		let second = Registration {
			device_id: Some("d".into()),
			..first
		};
		set_cache(second);
		assert_eq!(
			load().await.unwrap().unwrap().device_id.as_deref(),
			Some("d")
		);

		*CACHE.write().expect("registration cache poisoned") = None;
	}

	#[tokio::test]
	async fn store_and_load_from_dir_roundtrip() {
		let dir = tempfile::tempdir().unwrap();
		assert!(load_from(dir.path()).await.unwrap().is_none());

		let reg = sample();
		store_in(dir.path(), &reg).await.unwrap();

		let back = load_from(dir.path()).await.unwrap().unwrap();
		assert_eq!(back.server_id, reg.server_id);
		assert_eq!(back.device_key, reg.device_key);

		// File must not contain the plaintext key.
		let raw = std::fs::read(registration_file(dir.path())).unwrap();
		assert!(
			!raw.windows(b"PRIVATE KEY".len())
				.any(|w| w == b"PRIVATE KEY"),
			"registration file should be encrypted"
		);
	}

	#[tokio::test]
	async fn delete_in_removes_registration_and_reports_presence() {
		let dir = tempfile::tempdir().unwrap();
		assert!(
			!delete_in(dir.path()).await.unwrap(),
			"deleting when absent reports nothing removed"
		);

		store_in(dir.path(), &sample()).await.unwrap();
		assert!(registration_file(dir.path()).exists());

		assert!(
			delete_in(dir.path()).await.unwrap(),
			"deleting an existing registration reports it was removed"
		);
		assert!(!registration_file(dir.path()).exists());
		assert!(load_from(dir.path()).await.unwrap().is_none());
	}

	#[tokio::test]
	async fn store_uses_cheap_work_factor() {
		let dir = tempfile::tempdir().unwrap();
		store_in(dir.path(), &sample()).await.unwrap();

		let raw = std::fs::read(registration_file(dir.path())).unwrap();
		assert_eq!(scrypt_work_factor(&raw), Some(WORK_FACTOR));
	}

	#[tokio::test]
	async fn load_reencrypts_expensive_files() {
		let dir = tempfile::tempdir().unwrap();
		let path = registration_file(dir.path());
		let reg = sample();

		// Simulate a file written before the work factor was fixed (one notch
		// up, to keep the test fast).
		let machine_id = machine_uid::get().unwrap();
		let expensive =
			Passphrase::with_work_factor(derive_passphrase(&machine_id).into(), WORK_FACTOR + 1);
		let blob = encrypt_bytes(&serde_json::to_vec(&reg).unwrap(), expensive).unwrap();
		write_atomic(&path, &blob).await.unwrap();
		assert_eq!(scrypt_work_factor(&blob), Some(WORK_FACTOR + 1));

		let back = load_from(dir.path()).await.unwrap().unwrap();
		assert_eq!(back.server_id, reg.server_id);
		assert_eq!(back.device_key, reg.device_key);

		let raw = std::fs::read(&path).unwrap();
		assert_eq!(scrypt_work_factor(&raw), Some(WORK_FACTOR));
		let again = load_from(dir.path()).await.unwrap().unwrap();
		assert_eq!(again.server_id, reg.server_id);
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn store_writes_group_readable_file() {
		let dir = tempfile::tempdir().unwrap();
		store_in(dir.path(), &sample()).await.unwrap();

		let mode = std::fs::metadata(registration_file(dir.path()))
			.unwrap()
			.permissions()
			.mode() & 0o777;
		assert_eq!(mode, FILE_MODE, "expected {FILE_MODE:o}, got {mode:o}");
	}

	/// A group this process may chown to, other than `exclude`; `None` when it
	/// belongs to no other group and so can't set up the mismatch.
	#[cfg(unix)]
	fn other_gid(exclude: u32) -> Option<u32> {
		let out = std::process::Command::new("id").arg("-G").output().ok()?;
		String::from_utf8(out.stdout)
			.ok()?
			.split_whitespace()
			.filter_map(|gid| gid.parse().ok())
			.find(|gid| *gid != exclude)
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn store_writes_file_with_the_directory_group() {
		let dir = tempfile::tempdir().unwrap();
		let dir_gid = std::fs::metadata(dir.path()).unwrap().gid();
		let Some(shared) = other_gid(dir_gid) else {
			return;
		};
		std::os::unix::fs::chown(dir.path(), None, Some(shared)).unwrap();

		store_in(dir.path(), &sample()).await.unwrap();

		let gid = std::fs::metadata(registration_file(dir.path()))
			.unwrap()
			.gid();
		assert_eq!(gid, shared, "expected group {shared}, got {gid}");
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn load_repairs_group_of_files_written_elsewhere() {
		let dir = tempfile::tempdir().unwrap();
		store_in(dir.path(), &sample()).await.unwrap();
		let path = registration_file(dir.path());

		let dir_gid = std::fs::metadata(dir.path()).unwrap().gid();
		let Some(other) = other_gid(dir_gid) else {
			return;
		};
		std::os::unix::fs::chown(&path, None, Some(other)).unwrap();

		load_from(dir.path()).await.unwrap().unwrap();

		let gid = std::fs::metadata(&path).unwrap().gid();
		assert_eq!(gid, dir_gid, "expected group {dir_gid}, got {gid}");
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn load_repairs_mode_of_old_files() {
		let dir = tempfile::tempdir().unwrap();
		store_in(dir.path(), &sample()).await.unwrap();

		let path = registration_file(dir.path());
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

		load_from(dir.path()).await.unwrap().unwrap();
		let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
		assert_eq!(mode, FILE_MODE, "expected {FILE_MODE:o}, got {mode:o}");
	}
}
