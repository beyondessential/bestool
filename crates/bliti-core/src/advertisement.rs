//! What a device broadcasts, and how a client reads it back.
//!
//! Behaviour is specified in `.workhorse/specs/bliti/discovery.md` (BLI-ADV). The advertisement
//! carries the service UUID; the local name carries the handle, the rotation salt, and the version
//! marker, rendered as base32.
//!
//! This lives in the core rather than in the daemon because both ends need it: the device renders
//! the name and the client parses it, and the client is the web application, which compiles this
//! crate to wasm. One implementation means the two cannot drift.

use data_encoding::BASE32_NOPAD;

use crate::key_schedule::{HANDLE_LEN, Handle, ROTATION_SALT_LEN, RotationSalt, VERSION};

/// A legacy advertising payload carries 31 bytes.
pub const ADVERTISING_BUDGET: usize = 31;

/// Every advertising data element costs a length byte and a type byte before its content.
const AD_HEADER: usize = 2;

/// The mandatory flags element: header plus one byte of flags.
const FLAGS_LEN: usize = AD_HEADER + 1;

/// A 128-bit service UUID element: header plus sixteen bytes.
const SERVICE_UUID_LEN: usize = AD_HEADER + 16;

/// The raw payload: handle, salt, version.
pub const PAYLOAD_LEN: usize = HANDLE_LEN + ROTATION_SALT_LEN + 1;

/// The payload rendered as unpadded base32, which is what the local name holds.
pub const LOCAL_NAME_LEN: usize = (PAYLOAD_LEN * 8).div_ceil(5);

/// The advertising budget is a compile-time guarantee rather than something a test happens to check:
/// the flags and the service UUID must fit one legacy advertisement, and the name element must fit
/// one too, since a host places it in the scan response. Growing the payload past what fits breaks
/// the build rather than a device in the field.
const _: () = {
	assert!(FLAGS_LEN + SERVICE_UUID_LEN <= ADVERTISING_BUDGET);
	assert!(AD_HEADER + LOCAL_NAME_LEN <= ADVERTISING_BUDGET);
};

/// What a device advertises: the handle, the salt it was computed under, and the version marker.
///
/// A client reads the version before recomputing, so that a device speaking a version the client
/// does not hold is reported as exactly that rather than as silence (BLI-ADV, "Matching").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advertised {
	/// The advertised handle.
	pub handle: Handle,
	/// The rotation salt the handle was computed under.
	pub salt: RotationSalt,
	/// The version marker.
	pub version: u8,
}

impl Advertised {
	/// The payload for a handle and salt at the current version.
	pub fn new(handle: Handle, salt: RotationSalt) -> Self {
		Self {
			handle,
			salt,
			version: VERSION,
		}
	}

	/// The raw payload bytes: handle, then salt, then version.
	pub fn to_bytes(self) -> [u8; PAYLOAD_LEN] {
		let mut bytes = [0u8; PAYLOAD_LEN];
		bytes[..HANDLE_LEN].copy_from_slice(self.handle.as_bytes());
		bytes[HANDLE_LEN..HANDLE_LEN + ROTATION_SALT_LEN].copy_from_slice(self.salt.as_bytes());
		bytes[PAYLOAD_LEN - 1] = self.version;
		bytes
	}

	/// The local name a device advertises: the payload as unpadded base32.
	pub fn to_local_name(self) -> String {
		BASE32_NOPAD.encode(&self.to_bytes())
	}

	/// Read a payload from raw bytes. `None` where it is not the right shape.
	pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
		if bytes.len() != PAYLOAD_LEN {
			return None;
		}
		let handle: [u8; HANDLE_LEN] = bytes[..HANDLE_LEN].try_into().ok()?;
		let salt: [u8; ROTATION_SALT_LEN] = bytes[HANDLE_LEN..HANDLE_LEN + ROTATION_SALT_LEN]
			.try_into()
			.ok()?;
		Some(Self {
			handle: Handle::from_bytes(handle),
			salt: RotationSalt::from_bytes(salt),
			version: bytes[PAYLOAD_LEN - 1],
		})
	}

	/// Read a payload from a local name heard on the air.
	///
	/// `None` where the name is not a bliti payload at all, which is the ordinary case for every other
	/// device in range and is passed over rather than reported.
	pub fn from_local_name(name: &str) -> Option<Self> {
		if name.len() != LOCAL_NAME_LEN {
			return None;
		}
		let bytes = BASE32_NOPAD.decode(name.as_bytes()).ok()?;
		Self::from_bytes(&bytes)
	}

	/// Whether this advertisement belongs to the device holding `secret`.
	///
	/// One fast hash per advertisement heard per sticker held. The caller checks the version first,
	/// because no two versions produce a matching handle and silence would not say which it was.
	pub fn matches(self, secret: &crate::key_schedule::StickerSecret) -> bool {
		secret.handle(self.salt) == self.handle
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::key_schedule::StickerSecret;

	fn handle(first: u8) -> Handle {
		Handle::from_bytes([first, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88])
	}

	#[test]
	fn the_payload_is_the_stated_shape() {
		// That it fits the budget is asserted at compile time above; these are the sizes the spec
		// states, which a client in another language has to agree with.
		assert_eq!(FLAGS_LEN + SERVICE_UUID_LEN, 21);
		assert_eq!(PAYLOAD_LEN, 13);
		assert_eq!(LOCAL_NAME_LEN, 21);
	}

	#[test]
	fn the_rendering_is_the_stated_length() {
		let name =
			Advertised::new(handle(0x11), RotationSalt::from_bytes([1, 2, 3, 4])).to_local_name();
		assert_eq!(name.len(), LOCAL_NAME_LEN);
	}

	#[test]
	fn a_local_name_round_trips() {
		let advertised = Advertised::new(handle(0xab), RotationSalt::from_bytes([9, 8, 7, 6]));
		assert_eq!(
			Advertised::from_local_name(&advertised.to_local_name()),
			Some(advertised)
		);
	}

	#[test]
	fn the_version_marker_is_carried_and_readable_when_unsupported() {
		let advertised = Advertised::new(handle(0x01), RotationSalt::from_bytes([0; 4]));
		assert_eq!(advertised.to_bytes()[PAYLOAD_LEN - 1], VERSION);

		// A client must be able to read a version it does not hold, which is what separates "a device
		// at an unsupported version" from hearing nothing at all.
		let mut bytes = advertised.to_bytes();
		bytes[PAYLOAD_LEN - 1] = 99;
		let foreign = Advertised::from_local_name(&BASE32_NOPAD.encode(&bytes)).unwrap();
		assert_eq!(foreign.version, 99);
	}

	#[test]
	fn a_name_that_is_not_a_payload_is_passed_over() {
		// Every other device in range has a name of its own; none of them is a bliti device.
		for name in ["", "ATC_ORANGE", "Aranet4 2E4A5", "athom-co2-sen-b34960"] {
			assert_eq!(Advertised::from_local_name(name), None);
		}
		// The right length but not base32.
		assert_eq!(
			Advertised::from_local_name(&"!".repeat(LOCAL_NAME_LEN)),
			None
		);
	}

	#[test]
	fn a_client_matches_only_the_device_whose_sticker_it_holds() {
		let ours = StickerSecret::from_bytes([0x5a; 32]);
		let theirs = StickerSecret::from_bytes([0x5b; 32]);
		let salt = RotationSalt::from_bytes([4, 3, 2, 1]);

		let advertised = Advertised::new(ours.handle(salt), salt);
		assert!(advertised.matches(&ours));
		assert!(!advertised.matches(&theirs));
	}

	#[test]
	fn a_device_is_recognised_across_a_salt_change() {
		let ours = StickerSecret::from_bytes([0x77; 32]);
		let first = RotationSalt::from_bytes([0, 0, 0, 1]);
		let second = RotationSalt::from_bytes([0, 0, 0, 2]);

		let before = Advertised::new(ours.handle(first), first);
		let after = Advertised::new(ours.handle(second), second);

		// An observer without the sticker sees two unrelated handles.
		assert_ne!(before.handle, after.handle);
		// A client holding it recognises both.
		assert!(before.matches(&ours));
		assert!(after.matches(&ours));
	}
}
