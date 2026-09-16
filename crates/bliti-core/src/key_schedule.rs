//! The key schedule: the two derivations that take a board ID to the sticker secret and the
//! advertised handle.
//!
//! Behaviour is specified in `.workhorse/specs/bliti/key-schedule.md` (BLI-KEY). Both derivation
//! constants here are public: they are compiled into the device, the sticker generator, and every
//! client, and publishing them weakens nothing because neither derivation runs backwards. What they
//! provide is domain separation.
//!
//! Everything a sticker depends on is versioned together under [`VERSION`]. The constants, the
//! argon2id parameters, the source precedence and the encoding below, the pinned Endorsement Key
//! template, and the handle length are all covered by it; any of them changing is a new version,
//! because any of them changing changes the secret.

use crate::board_id::SourceKind;

/// The version marker carried in the QR payload (BLI-STK) and in the advertisement (BLI-ADV).
///
/// It covers everything a sticker depends on. A change that leaves the secret identical does not
/// move it, because moving it orphans every sticker already fixed to an enclosure.
pub const VERSION: u8 = 1;

/// The fixed argon2id salt for the sticker-secret derivation. Public and versioned. Only the
/// memory-hard derivation uses it, so it is gated with that.
#[cfg(feature = "derive")]
const STICKER_SECRET_SALT: [u8; 16] = [
	0x3e, 0xdf, 0xe9, 0x5c, 0xeb, 0x86, 0xfa, 0xdd, 0x23, 0xd4, 0x6a, 0x87, 0x34, 0xc7, 0xeb, 0x13,
];

/// The fixed domain-separation constant for the handle derivation. Public and versioned.
const HANDLE_CONSTANT: [u8; 16] = [
	0x15, 0x9f, 0x0a, 0x92, 0x9c, 0x9d, 0x0b, 0x80, 0x41, 0x7e, 0x9b, 0x87, 0x75, 0xbb, 0x18, 0x39,
];

/// The argon2id memory parameter, in kibibytes: 2 GiB. Part of the derivation, not a tuning choice.
pub const STICKER_SECRET_MEMORY_KIB: u32 = 2 * 1024 * 1024;

/// The argon2id memory parameter in bytes, for a device to check against its free memory before
/// beginning (BLI-KEY, "Deriving on the device").
pub const STICKER_SECRET_MEMORY_BYTES: u64 = (STICKER_SECRET_MEMORY_KIB as u64) * 1024;

/// The argon2id pass count. Part of the derivation.
pub const STICKER_SECRET_PASSES: u32 = 1;

/// The argon2id lane count. Part of the derivation. Whether the lanes are computed concurrently or
/// in sequence does not change the result.
pub const STICKER_SECRET_LANES: u32 = 2;

/// The length of a sticker secret in bytes.
pub const STICKER_SECRET_LEN: usize = 32;

/// The length of an advertised handle in bytes: eight, which makes a collision between two devices
/// at one site implausible and fits the advertising budget in BLI-ADV.
pub const HANDLE_LEN: usize = 8;

/// The length of the rotation salt in bytes.
pub const ROTATION_SALT_LEN: usize = 4;

/// A sticker secret: the value printed in the QR code, and the only secret in the system.
#[derive(Clone, PartialEq, Eq)]
pub struct StickerSecret([u8; STICKER_SECRET_LEN]);

impl StickerSecret {
	/// Wrap raw bytes as a sticker secret, as read from a QR payload by a client.
	pub fn from_bytes(bytes: [u8; STICKER_SECRET_LEN]) -> Self {
		Self(bytes)
	}

	/// The raw bytes of the secret.
	pub fn as_bytes(&self) -> &[u8; STICKER_SECRET_LEN] {
		&self.0
	}

	/// Derive the advertised handle for a given rotation salt (BLI-KEY, "Advertised handle").
	///
	/// This is a fast keyed hash, deliberately cheap: a client recomputes it for every advertisement
	/// it hears against every sticker it holds, so a memory-hard function here would be felt during
	/// scanning. It runs in the browser, where the memory-hard derivation never does.
	pub fn handle(&self, salt: RotationSalt) -> Handle {
		let mut data = [0u8; HANDLE_CONSTANT.len() + ROTATION_SALT_LEN];
		data[..HANDLE_CONSTANT.len()].copy_from_slice(&HANDLE_CONSTANT);
		data[HANDLE_CONSTANT.len()..].copy_from_slice(&salt.0);
		let digest = blake3::keyed_hash(&self.0, &data);
		let mut handle = [0u8; HANDLE_LEN];
		handle.copy_from_slice(&digest.as_bytes()[..HANDLE_LEN]);
		Handle(handle)
	}
}

impl core::fmt::Debug for StickerSecret {
	fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
		// A sticker secret is a credential; never render it in a debug log.
		f.write_str("StickerSecret(..)")
	}
}

/// An advertised handle: the eight-byte value a device broadcasts and a client recomputes to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Handle([u8; HANDLE_LEN]);

impl Handle {
	/// The raw bytes of the handle.
	pub fn as_bytes(&self) -> &[u8; HANDLE_LEN] {
		&self.0
	}
}

/// The rotation salt: a short random value advertised in the clear that changes every fifteen
/// minutes (BLI-ADV, "Rotation"), so a passive observer cannot follow a device by its handle alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RotationSalt([u8; ROTATION_SALT_LEN]);

impl RotationSalt {
	/// Wrap raw bytes as a rotation salt, as observed in an advertisement.
	pub fn from_bytes(bytes: [u8; ROTATION_SALT_LEN]) -> Self {
		Self(bytes)
	}

	/// The raw bytes of the salt.
	pub fn as_bytes(&self) -> &[u8; ROTATION_SALT_LEN] {
		&self.0
	}
}

/// The argon2id password for a board ID: a source tag byte followed by the raw bytes of the source
/// value, most significant first (BLI-KEY, "What is derived from").
///
/// The tag identifies which kind of source the value came from, so a value byte-identical across two
/// kinds of source still derives differently. Lengths are fixed per kind of source, so the tag
/// leaves the input unambiguous without a length prefix.
pub fn argon2_password(kind: SourceKind, raw: &[u8]) -> Vec<u8> {
	let mut password = Vec::with_capacity(1 + raw.len());
	password.push(kind.tag());
	password.extend_from_slice(raw);
	password
}

/// Whether a device has room to run the sticker-secret derivation, given the bytes of memory it has
/// available. The derivation needs its full memory parameter at once and is killed by the operating
/// system rather than told the allocation failed, so a device checks this before beginning (BLI-KEY,
/// "Deriving on the device").
pub fn check_memory(available_bytes: u64) -> Result<(), KeyError> {
	if available_bytes < STICKER_SECRET_MEMORY_BYTES {
		return Err(KeyError::InsufficientMemory {
			required: STICKER_SECRET_MEMORY_BYTES,
			available: available_bytes,
		});
	}
	Ok(())
}

/// Derive the sticker secret from a board ID with argon2id under the fixed constant (BLI-KEY,
/// "Sticker secret").
///
/// This is the memory-hard derivation. It runs on the device and in the sticker generator, never in
/// a client, and is behind the `derive` feature so a wasm build does not pull argon2. A device
/// checks [`check_memory`] before calling this, because the allocation cannot fail gracefully.
#[cfg(feature = "derive")]
pub fn derive_sticker_secret(
	board_id: &crate::board_id::BoardId,
) -> Result<StickerSecret, KeyError> {
	let password = argon2_password(board_id.kind(), board_id.raw());
	derive_sticker_secret_with(
		STICKER_SECRET_MEMORY_KIB,
		STICKER_SECRET_PASSES,
		STICKER_SECRET_LANES,
		&password,
	)
}

/// Run argon2id over a password with the fixed salt and given parameters. Split out so a
/// known-answer test can pin the wiring, the salt, and the password encoding cheaply with small
/// parameters, while the production parameters are pinned separately by their constants.
#[cfg(feature = "derive")]
fn derive_sticker_secret_with(
	memory_kib: u32,
	passes: u32,
	lanes: u32,
	password: &[u8],
) -> Result<StickerSecret, KeyError> {
	use argon2::{Algorithm, Argon2, Params, Version};

	let params = Params::new(memory_kib, passes, lanes, Some(STICKER_SECRET_LEN))
		.map_err(|err| KeyError::Parameters(err.to_string()))?;
	let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
	let mut out = [0u8; STICKER_SECRET_LEN];
	argon2
		.hash_password_into(password, &STICKER_SECRET_SALT, &mut out)
		.map_err(|err| KeyError::Derivation(err.to_string()))?;
	Ok(StickerSecret(out))
}

/// A failure in the key schedule.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
	/// A device does not have room for the memory-hard derivation.
	#[error(
		"insufficient memory for the sticker-secret derivation: needs {required} bytes, {available} available"
	)]
	InsufficientMemory {
		/// Bytes the derivation needs at once.
		required: u64,
		/// Bytes the device has available.
		available: u64,
	},

	/// The argon2id parameters were rejected.
	#[error("invalid argon2 parameters: {0}")]
	Parameters(String),

	/// The argon2id derivation failed.
	#[error("sticker-secret derivation failed: {0}")]
	Derivation(String),
}

#[cfg(test)]
mod tests {
	use super::*;

	fn hex(bytes: &[u8]) -> String {
		bytes.iter().map(|b| format!("{b:02x}")).collect()
	}

	#[test]
	fn production_parameters_are_pinned() {
		// A guard that always runs: an accidental edit to the memory-hard parameters, which are
		// versioned into every sticker, fails here without allocating 2 GiB.
		assert_eq!(STICKER_SECRET_MEMORY_KIB, 2 * 1024 * 1024);
		assert_eq!(STICKER_SECRET_MEMORY_BYTES, 2 * 1024 * 1024 * 1024);
		assert_eq!(STICKER_SECRET_PASSES, 1);
		assert_eq!(STICKER_SECRET_LANES, 2);
		assert_eq!(STICKER_SECRET_LEN, 32);
		assert_eq!(HANDLE_LEN, 8);
		assert_eq!(VERSION, 1);
	}

	#[test]
	fn password_is_tag_then_raw_bytes() {
		let password = argon2_password(SourceKind::RaspberryPiSerial, &[0xf3, 0x75]);
		assert_eq!(
			password,
			vec![SourceKind::RaspberryPiSerial.tag(), 0xf3, 0x75]
		);
	}

	#[test]
	fn deriving_from_characters_differs_from_the_bytes_they_denote() {
		// The raw bytes are used, never a text rendering: a serial read as characters gives a
		// different password from the bytes those characters denote.
		let from_chars = argon2_password(SourceKind::RaspberryPiSerial, b"f375");
		let from_bytes = argon2_password(SourceKind::RaspberryPiSerial, &[0xf3, 0x75]);
		assert_ne!(from_chars, from_bytes);
	}

	#[test]
	fn identical_values_from_different_sources_derive_differently() {
		let value = [0x11, 0x22, 0x33, 0x44];
		let a = argon2_password(SourceKind::OneTimeProgrammable, &value);
		let b = argon2_password(SourceKind::SmbiosSystemUuid, &value);
		assert_ne!(a, b);
	}

	#[test]
	fn check_memory_reports_insufficient() {
		assert!(matches!(
			check_memory(STICKER_SECRET_MEMORY_BYTES - 1),
			Err(KeyError::InsufficientMemory { .. })
		));
		assert!(check_memory(STICKER_SECRET_MEMORY_BYTES).is_ok());
	}

	#[test]
	fn handle_known_answer() {
		// Pins the handle derivation: constant, keying, salt handling, and eight-byte truncation.
		let secret = StickerSecret::from_bytes([0x42; STICKER_SECRET_LEN]);
		let salt = RotationSalt::from_bytes([0x01, 0x02, 0x03, 0x04]);
		let handle = secret.handle(salt);
		assert_eq!(hex(handle.as_bytes()), "5a22650058575721");
	}

	#[test]
	fn handle_changes_with_the_salt() {
		let secret = StickerSecret::from_bytes([0x42; STICKER_SECRET_LEN]);
		let a = secret.handle(RotationSalt::from_bytes([0, 0, 0, 0]));
		let b = secret.handle(RotationSalt::from_bytes([0, 0, 0, 1]));
		assert_ne!(a, b);
	}

	#[cfg(feature = "derive")]
	#[test]
	fn sticker_secret_wiring_known_answer() {
		// A cheap known-answer test with small memory: pins the algorithm, version, salt constant,
		// password encoding, and output length. The memory-hard magnitude is pinned separately by
		// `production_parameters_are_pinned`, so together they cover the whole derivation.
		let password = argon2_password(SourceKind::RaspberryPiSerial, &[0xf3, 0x75, 0x65, 0x10]);
		let secret = derive_sticker_secret_with(32, 1, 2, &password).unwrap();
		assert_eq!(
			hex(secret.as_bytes()),
			"6f1389914fdb010c7ed6f41278bf0dd2ee97f0fd692e8bae7c3f581c06ef610c"
		);
	}

	#[cfg(feature = "derive")]
	#[test]
	#[ignore = "allocates 2 GiB and runs the full derivation; run explicitly with --ignored"]
	fn sticker_secret_production_known_answer() {
		use crate::board_id::BoardId;
		let board_id = BoardId::new(
			SourceKind::RaspberryPiSerial,
			vec![0xf3, 0x75, 0x65, 0x10, 0xf6, 0x32, 0xcf, 0xad],
		)
		.unwrap();
		let secret = derive_sticker_secret(&board_id).unwrap();
		assert_eq!(
			hex(secret.as_bytes()),
			"cb89bf939b867ec6e15530a6b92db98a14f170e1f4c9ff218cbd460e2140ccbd"
		);
	}
}
