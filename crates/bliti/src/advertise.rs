//! What a device broadcasts, and how it fits the advertising budget.
//!
//! Behaviour is specified in BLI-ADV. The advertisement carries the service UUID and a local name;
//! the scan response carries service data holding the handle, the rotation salt, and the version
//! marker. Splitting the content this way is what fits it into the two 31-byte budgets, so the
//! arithmetic below is load-bearing rather than incidental, and is checked by tests.
//!
//! This module holds the payload shapes and the budget arithmetic, with no BlueZ in it, so the
//! budgets can be verified without an adapter.

use bliti_core::key_schedule::{HANDLE_LEN, Handle, ROTATION_SALT_LEN, RotationSalt, VERSION};

/// A legacy advertising payload carries 31 bytes.
pub const ADVERTISING_BUDGET: usize = 31;

/// Every advertising data element costs a length byte and a type byte before its content.
const AD_HEADER: usize = 2;

/// The mandatory flags element: header plus one byte of flags.
const FLAGS_LEN: usize = AD_HEADER + 1;

/// A 128-bit service UUID element: header plus sixteen bytes.
const SERVICE_UUID_LEN: usize = AD_HEADER + 16;

/// A service data element keyed by a 128-bit UUID costs its header and the UUID before any content.
const SERVICE_DATA_OVERHEAD: usize = AD_HEADER + 16;

/// What is left in the advertisement for a local name, once flags and the service UUID are paid for.
pub const LOCAL_NAME_BUDGET: usize = ADVERTISING_BUDGET - FLAGS_LEN - SERVICE_UUID_LEN - AD_HEADER;

/// What is left in the scan response for service data content.
pub const SERVICE_DATA_BUDGET: usize = ADVERTISING_BUDGET - SERVICE_DATA_OVERHEAD;

/// How many bytes of the handle the local name renders: as many as fit the budget two hexadecimal
/// characters at a time. A rendering of the whole handle does not fit.
const LOCAL_NAME_HANDLE_BYTES: usize = LOCAL_NAME_BUDGET / 2;

/// The service data a device advertises: the handle, the salt it was computed under, and the version.
///
/// A client reads the version before recomputing, so that a device speaking a version the client does
/// not hold is reported as exactly that rather than as silence (BLI-ADV, "Matching").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanPayload {
	/// The advertised handle.
	pub handle: Handle,
	/// The rotation salt the handle was computed under.
	pub salt: RotationSalt,
	/// The version marker.
	pub version: u8,
}

impl ScanPayload {
	/// The payload for a handle and salt at the current version.
	pub fn new(handle: Handle, salt: RotationSalt) -> Self {
		Self {
			handle,
			salt,
			version: VERSION,
		}
	}

	/// The service data bytes: handle, then salt, then version. Exactly [`SERVICE_DATA_BUDGET`].
	pub fn to_service_data(self) -> Vec<u8> {
		let mut data = Vec::with_capacity(SERVICE_DATA_BUDGET);
		data.extend_from_slice(self.handle.as_bytes());
		data.extend_from_slice(self.salt.as_bytes());
		data.push(self.version);
		data
	}

	/// Read service data as heard from a device. `None` where it is not the right shape.
	pub fn parse(data: &[u8]) -> Option<Self> {
		if data.len() != SERVICE_DATA_BUDGET {
			return None;
		}
		let handle: [u8; HANDLE_LEN] = data[..HANDLE_LEN].try_into().ok()?;
		let salt: [u8; ROTATION_SALT_LEN] = data[HANDLE_LEN..HANDLE_LEN + ROTATION_SALT_LEN]
			.try_into()
			.ok()?;
		Some(Self {
			handle: Handle::from_bytes(handle),
			salt: RotationSalt::from_bytes(salt),
			version: data[HANDLE_LEN + ROTATION_SALT_LEN],
		})
	}
}

/// The local name a device advertises: the first four bytes of the handle as eight hexadecimal
/// characters.
///
/// This costs nothing, because the handle is not secret, and it gives a client platform that can only
/// filter by name prefix something to filter on, and a human something to match by eye. It changes
/// when the salt rolls, because the handle does.
pub fn local_name(handle: Handle) -> String {
	handle.as_bytes()[..LOCAL_NAME_HANDLE_BYTES]
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn handle(first: u8) -> Handle {
		Handle::from_bytes([first, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88])
	}

	#[test]
	fn the_budget_arithmetic_is_what_the_spec_states() {
		// Flags take three bytes and a 128-bit service UUID eighteen, leaving ten in the
		// advertisement; service data keyed by a 128-bit UUID takes eighteen, leaving thirteen.
		assert_eq!(FLAGS_LEN, 3);
		assert_eq!(SERVICE_UUID_LEN, 18);
		assert_eq!(LOCAL_NAME_BUDGET, 8);
		assert_eq!(SERVICE_DATA_BUDGET, 13);
	}

	#[test]
	fn service_data_fills_the_scan_response_exactly() {
		// The handle is eight bytes, the salt four, and the version one: thirteen exactly.
		let payload = ScanPayload::new(handle(0x11), RotationSalt::from_bytes([1, 2, 3, 4]));
		let data = payload.to_service_data();
		assert_eq!(data.len(), SERVICE_DATA_BUDGET);
		assert_eq!(SERVICE_DATA_OVERHEAD + data.len(), ADVERTISING_BUDGET);
		assert_eq!(HANDLE_LEN + ROTATION_SALT_LEN + 1, SERVICE_DATA_BUDGET);
	}

	#[test]
	fn the_advertisement_fits_its_own_budget() {
		let name = local_name(handle(0x11));
		assert_eq!(name.len(), LOCAL_NAME_BUDGET);
		assert_eq!(
			FLAGS_LEN + SERVICE_UUID_LEN + AD_HEADER + name.len(),
			ADVERTISING_BUDGET
		);
	}

	#[test]
	fn service_data_round_trips() {
		let payload = ScanPayload::new(handle(0xab), RotationSalt::from_bytes([9, 8, 7, 6]));
		assert_eq!(
			ScanPayload::parse(&payload.to_service_data()),
			Some(payload)
		);
	}

	#[test]
	fn the_version_marker_is_carried_in_the_advertisement() {
		let payload = ScanPayload::new(handle(0x01), RotationSalt::from_bytes([0; 4]));
		let data = payload.to_service_data();
		assert_eq!(*data.last().unwrap(), VERSION);
		// A client can read a version it does not hold, which is what separates "a device at an
		// unsupported version" from hearing nothing at all.
		let mut foreign = data.clone();
		*foreign.last_mut().unwrap() = 99;
		assert_eq!(ScanPayload::parse(&foreign).unwrap().version, 99);
	}

	#[test]
	fn service_data_of_the_wrong_shape_is_rejected() {
		assert_eq!(ScanPayload::parse(&[]), None);
		assert_eq!(ScanPayload::parse(&[0u8; SERVICE_DATA_BUDGET - 1]), None);
		assert_eq!(ScanPayload::parse(&[0u8; SERVICE_DATA_BUDGET + 1]), None);
	}

	#[test]
	fn the_local_name_is_the_first_four_handle_bytes_and_follows_the_salt() {
		assert_eq!(local_name(handle(0x11)), "11223344");
		// The handle changes when the salt rolls, so the name a client filters on changes with it.
		assert_ne!(local_name(handle(0x11)), local_name(handle(0xff)));
	}
}
