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

use algae_cli::{
	passphrases::Passphrase,
	streams::{decrypt_stream, encrypt_stream},
};
use base64::{
	Engine as _,
	engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD},
};
use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

const VERSION: &str = "registration-1";

/// Environment variable overriding the base directory for the registration
/// file. Set by tests and honoured for ad-hoc relocation; when set, legacy
/// migration is skipped.
const DIR_ENV: &str = "BESTOOL_CANOPY_DIR";

/// blake3 KDF context string for the machine-id-derived file passphrase. Bump
/// the version suffix if the derivation ever changes.
const KDF_CONTEXT: &str = "bestool canopy-registration v1 (machine-id)";

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

/// Default base directory for the registration file (honours [`DIR_ENV`]).
///
/// Uses the platform convention for machine-global state: `/etc` on Linux,
/// `%ProgramData%` on Windows.
pub fn default_dir() -> PathBuf {
	if let Some(dir) = std::env::var_os(DIR_ENV) {
		return PathBuf::from(dir);
	}
	#[cfg(windows)]
	{
		let base = std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
		PathBuf::from(base).join("bestool")
	}
	#[cfg(not(windows))]
	{
		PathBuf::from("/etc/bestool")
	}
}

fn registration_file(dir: &Path) -> PathBuf {
	dir.join("canopy-registration")
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

/// Process-wide cache of a successfully loaded registration, so repeated
/// reporting reads (e.g. the doctor tick) don't re-run scrypt each time. Only
/// populated on a hit; a fresh enrollment is picked up on the next process
/// start.
static CACHE: tokio::sync::OnceCell<Registration> = tokio::sync::OnceCell::const_new();

/// Load the registration from the default location.
///
/// If the file is absent, migrates from the legacy `/etc/tamanu` plaintext
/// files (unless [`DIR_ENV`] is set). Returns `None` when there's nothing to
/// load.
pub async fn load() -> Result<Option<Registration>> {
	if let Some(reg) = CACHE.get() {
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
		let _ = CACHE.set(reg.clone());
	}
	Ok(reg)
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
pub async fn store(reg: &Registration) -> Result<()> {
	store_in(&default_dir(), reg).await
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
	let bytes = tokio::fs::read(path)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("reading {}", path.display()))?;
	let plaintext = decrypt_bytes(&bytes, machine_passphrase()?)
		.wrap_err("decrypting registration (was this disk cloned from another machine?)")?;
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

/// Build the passphrase that unlocks the local registration file from the
/// host's machine id, read via the `machine-uid` crate (machine-id on Linux,
/// MachineGuid on Windows, IOPlatformUUID on macOS). A TPM, where one is
/// present, could augment this by sealing the key in hardware; hosts without a
/// TPM keep using the machine id.
fn machine_passphrase() -> Result<Passphrase> {
	let id =
		machine_uid::get().map_err(|err| miette!("could not read the host machine id: {err}"))?;
	Ok(Passphrase::new(derive_passphrase(&id).into()))
}

fn derive_passphrase(machine_id: &str) -> String {
	let key = blake3::derive_key(KDF_CONTEXT, machine_id.as_bytes());
	STANDARD_NO_PAD.encode(key)
}

// algae's stream API takes `Box<dyn Identity>` (not `Send`), which would poison
// the `Send` futures the reporting path requires. The payload is tiny and fully
// in-memory (no tokio reactor needed), so we drive algae to completion on the
// current thread with `block_on` inside a synchronous helper — nothing
// non-`Send` is then held across an `.await` in the async callers.
fn encrypt_bytes(plaintext: &[u8], passphrase: Passphrase) -> Result<Vec<u8>> {
	futures::executor::block_on(async {
		let mut out = futures::io::Cursor::new(Vec::new());
		encrypt_stream(plaintext, &mut out, Box::new(passphrase))
			.await
			.wrap_err("encrypting registration")?;
		Ok(out.into_inner())
	})
}

fn decrypt_bytes(ciphertext: &[u8], passphrase: Passphrase) -> Result<Vec<u8>> {
	futures::executor::block_on(async {
		let reader = futures::io::Cursor::new(ciphertext.to_vec());
		let mut out: Vec<u8> = Vec::new();
		decrypt_stream(reader, &mut out, Box::new(passphrase))
			.await
			.wrap_err("decrypting registration")?;
		Ok(out)
	})
}

async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
	let tmp = path.with_extension("tmp");
	let mut opts = tokio::fs::OpenOptions::new();
	opts.write(true).create(true).truncate(true);
	#[cfg(windows)]
	{
		const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
		opts.attributes(FILE_ATTRIBUTE_HIDDEN);
	}
	#[cfg(unix)]
	{
		opts.mode(0o600);
	}
	let mut f = opts
		.open(&tmp)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("creating {}", tmp.display()))?;
	use tokio::io::AsyncWriteExt as _;
	f.write_all(bytes).await.into_diagnostic()?;
	f.sync_all().await.into_diagnostic()?;
	drop(f);

	tokio::fs::rename(&tmp, path)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("renaming into {}", path.display()))
}

#[cfg(test)]
mod tests {
	use super::*;

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

	#[test]
	fn derive_passphrase_is_stable_and_machine_specific() {
		assert_eq!(
			derive_passphrase("machine-aaaa"),
			derive_passphrase("machine-aaaa")
		);
		assert_ne!(
			derive_passphrase("machine-aaaa"),
			derive_passphrase("machine-bbbb")
		);
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
}
