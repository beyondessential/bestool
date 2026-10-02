//! Machine-bound encrypted files: the primitives the registration store and the
//! certificate key store are both built from.
//!
//! Encryption is algae (the age/scrypt profile this workspace already uses for
//! `protect`/`reveal` and the enrollment ticket), keyed by a passphrase derived
//! from the host's machine id. A cloned disk can't reuse what the file carries
//! on a different machine, and nothing secret is at rest in plaintext.
//!
//! The machine-id binding is a deliberately weak, software-only measure. Where a
//! TPM is present it could augment this — sealing or deriving the unlock key in
//! hardware via [`machine_passphrase`] — while hosts without one keep using the
//! machine id, and neither the file format nor any consumer changes.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use algae_cli::{
	passphrases::Passphrase,
	streams::{decrypt_stream, encrypt_stream},
};
use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};

/// Environment variable overriding the base directory for the machine-bound
/// files. Set by tests and honoured for ad-hoc relocation; when set, legacy
/// migration is skipped.
pub const DIR_ENV: &str = "BESTOOL_CANOPY_DIR";

/// blake3 KDF context string for the machine-id-derived file passphrase. Bump
/// the version suffix if the derivation ever changes.
///
/// Named for the registration because that is the file it was introduced for,
/// and it must not change: every host's registration is already encrypted under
/// it. The certificate key store shares the derivation rather than inventing a
/// second one — both files sit on the same disk under the same threat model, so
/// separate contexts would buy nothing.
const KDF_CONTEXT: &str = "bestool canopy-registration v1 (machine-id)";

/// Unix mode for machine-bound files. Group-readable so unprivileged runs
/// sharing the daemon's group (e.g. `bestool tamanu doctor` run by hand) read
/// the same state instead of falling back to another source and rewriting it.
#[cfg(unix)]
pub const FILE_MODE: u32 = 0o640;

/// Unix mode for a machine-bound file nothing unprivileged reads.
///
/// [`FILE_MODE`]'s group read is granted for state an unprivileged run needs to
/// see. A file holding private keys is not that: the passphrase unlocking it is
/// derived from the machine id, which is world-readable on Linux, so group read
/// on the ciphertext is group access to the keys themselves. Owner only, and no
/// group inheritance either — the group the config directory carries is the one
/// being kept out.
#[cfg(unix)]
pub const PRIVATE_FILE_MODE: u32 = 0o600;

/// scrypt work factor (`N = 2^WORK_FACTOR`).
///
/// The machine passphrase is a 256-bit blake3-derived key, so scrypt's
/// memory-hardness adds no protection; age's default calibrates to ~1 second of
/// scrypt, which on a fast server is a 512MiB arena — enough to blow through a
/// service MemoryMax. 2^12 keeps the arena at 4MiB.
pub const WORK_FACTOR: u8 = 12;

/// Default base directory for machine-bound files (honours [`DIR_ENV`]).
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

/// Build the passphrase that unlocks a machine-bound file from the host's
/// machine id, read via the `machine-uid` crate (machine-id on Linux,
/// MachineGuid on Windows, IOPlatformUUID on macOS).
pub fn machine_passphrase() -> Result<Passphrase> {
	let id =
		machine_uid::get().map_err(|err| miette!("could not read the host machine id: {err}"))?;
	Ok(Passphrase::with_work_factor(
		derive_passphrase(&id).into(),
		WORK_FACTOR,
	))
}

pub(crate) fn derive_passphrase(machine_id: &str) -> String {
	let key = blake3::derive_key(KDF_CONTEXT, machine_id.as_bytes());
	STANDARD_NO_PAD.encode(key)
}

// algae's stream API takes `Box<dyn Identity>` (not `Send`), which would poison
// the `Send` futures the reporting path requires. The payload is tiny and fully
// in-memory (no tokio reactor needed), so we drive algae to completion on the
// current thread with `block_on` inside a synchronous helper — nothing
// non-`Send` is then held across an `.await` in the async callers.
pub fn encrypt_bytes(plaintext: &[u8], passphrase: Passphrase) -> Result<Vec<u8>> {
	futures::executor::block_on(async {
		let mut out = futures::io::Cursor::new(Vec::new());
		encrypt_stream(plaintext, &mut out, Box::new(passphrase))
			.await
			.wrap_err("encrypting")?;
		Ok(out.into_inner())
	})
}

pub fn decrypt_bytes(ciphertext: &[u8], passphrase: Passphrase) -> Result<Vec<u8>> {
	futures::executor::block_on(async {
		let reader = futures::io::Cursor::new(ciphertext.to_vec());
		let mut out: Vec<u8> = Vec::new();
		decrypt_stream(reader, &mut out, Box::new(passphrase))
			.await
			.wrap_err("decrypting")?;
		Ok(out)
	})
}

/// Extract the scrypt work factor (log_n) from an age file header.
///
/// The header is ASCII text even in the binary format: a version line, then
/// `-> scrypt <salt> <log_n>` for passphrase-encrypted files.
pub fn scrypt_work_factor(ciphertext: &[u8]) -> Option<u8> {
	ciphertext
		.split(|&b| b == b'\n')
		.take(2)
		.filter_map(|line| std::str::from_utf8(line).ok())
		.find_map(|line| line.strip_prefix("-> scrypt "))
		.and_then(|rest| rest.split_ascii_whitespace().nth(1))
		.and_then(|n| n.parse().ok())
}

/// Write `bytes` to `path` through a temporary file and a rename, so a reader
/// never sees a half-written file and a failed write leaves the previous
/// contents intact.
pub async fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
	write_atomic_with_mode(path, bytes, Sharing::WithDirGroup).await
}

/// [`write_atomic`], for a file only its owner may read.
///
/// Written [`PRIVATE_FILE_MODE`] and left with the group it was created under,
/// rather than the config directory's.
pub async fn write_atomic_private(path: &Path, bytes: &[u8]) -> Result<()> {
	write_atomic_with_mode(path, bytes, Sharing::OwnerOnly).await
}

/// Who a machine-bound file is readable by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sharing {
	/// [`FILE_MODE`], carrying the config directory's group.
	WithDirGroup,
	/// [`PRIVATE_FILE_MODE`], with no group inheritance.
	OwnerOnly,
}

impl Sharing {
	#[cfg(unix)]
	fn mode(self) -> u32 {
		match self {
			Self::WithDirGroup => FILE_MODE,
			Self::OwnerOnly => PRIVATE_FILE_MODE,
		}
	}
}

async fn write_atomic_with_mode(path: &Path, bytes: &[u8], sharing: Sharing) -> Result<()> {
	// A mode is a unix concern; on Windows what a file is readable by is the
	// directory's ACL inheritance, which this does not set either way.
	#[cfg(not(unix))]
	let _ = sharing;

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
		opts.mode(sharing.mode());
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

	// `mode()` only applies on creation and is filtered by the umask, so set
	// the permissions explicitly to cover pre-existing tmp files and
	// restrictive service umasks.
	#[cfg(unix)]
	tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(sharing.mode()))
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("setting permissions on {}", tmp.display()))?;
	#[cfg(unix)]
	if sharing == Sharing::WithDirGroup {
		inherit_dir_group(&tmp).await;
	}

	tokio::fs::rename(&tmp, path)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("renaming into {}", path.display()))
}

/// Remove `path`, reporting whether it was there to remove.
pub async fn remove_if_present(path: &Path) -> Result<bool> {
	match tokio::fs::remove_file(path).await {
		Ok(()) => Ok(true),
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
		Err(err) => Err(err)
			.into_diagnostic()
			.wrap_err_with(|| format!("removing {}", path.display())),
	}
}

/// Bring a machine-bound file's mode back to [`FILE_MODE`], for files written
/// before group read was granted.
///
/// Best-effort: only the owner can chmod, and unprivileged readers that get this
/// far don't need to.
#[cfg(unix)]
pub async fn repair_mode(path: &Path) {
	set_mode(path, FILE_MODE).await;
	inherit_dir_group(path).await;
}

/// Bring a private machine-bound file's mode back to [`PRIVATE_FILE_MODE`], for
/// one written before group read was taken away from it.
///
/// Narrows where [`repair_mode`] widens, and leaves the group alone: the point
/// is to take the shared group's read back off a file holding keys.
#[cfg(unix)]
pub async fn repair_mode_private(path: &Path) {
	set_mode(path, PRIVATE_FILE_MODE).await;
}

/// Best-effort: only the owner can chmod, and unprivileged readers that get this
/// far don't need to.
#[cfg(unix)]
async fn set_mode(path: &Path, mode: u32) {
	if let Ok(meta) = tokio::fs::metadata(path).await
		&& meta.permissions().mode() & 0o777 != mode
	{
		let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await;
	}
}

/// Give a directory the group of the one above it, for state kept in a
/// subdirectory of the config directory rather than directly in it.
///
/// Without this the chain of inheritance stops at the subdirectory: a setgid
/// parent propagates both its group and the bit, but a parent that is only
/// group-owned gives the new directory the creating process's group, and every
/// file under it then inherits that instead of the group meant to read them.
///
/// Best-effort, for the same reason as [`inherit_dir_group`].
#[cfg(unix)]
pub async fn inherit_parent_group(dir: &Path) {
	inherit_dir_group(dir).await;
}

/// Give `path` the group of the directory it sits in, so [`FILE_MODE`]'s group
/// read reaches the group owning the config directory. A setgid directory
/// confers it already; one without the bit does not. Best-effort, since chowning
/// needs ownership and a reader that can already open the file does not need it
/// to have worked.
#[cfg(unix)]
async fn inherit_dir_group(path: &Path) {
	use std::os::unix::fs::MetadataExt as _;

	let Some(dir) = path.parent() else { return };
	let (Ok(file), Ok(dir)) = (
		tokio::fs::metadata(path).await,
		tokio::fs::metadata(dir).await,
	) else {
		return;
	};

	if file.gid() != dir.gid()
		&& let Err(err) = std::os::unix::fs::chown(path, None, Some(dir.gid()))
	{
		tracing::debug!(path = %path.display(), %err, "could not set the file's group");
	}
}

#[cfg(test)]
mod tests {
	use super::*;

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
	async fn write_atomic_leaves_no_temp_file_behind() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("thing");
		write_atomic(&path, b"contents").await.unwrap();
		assert_eq!(std::fs::read(&path).unwrap(), b"contents");
		assert!(!path.with_extension("tmp").exists());
	}
}
