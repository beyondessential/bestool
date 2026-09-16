//! Customer-programmable one-time-programmable memory as a board ID source.
//!
//! Specified in BLI-BID, "Provisioned one-time-programmable memory": where the board carries
//! customer-programmable one-time-programmable memory that has been written, its contents are the
//! board ID.
//!
//! On Raspberry Pi hardware the kernel exposes the customer OTP region as an nvmem device. It is 32
//! bytes wide and reads as zeros on every unprogrammed board, which is the ordinary case and the
//! reason [`super::super::Presence::Placeholder`] exists: precedence falls through it to the serial
//! number rather than deriving a secret every unprogrammed board would share.

use std::{fs, path::PathBuf};

use crate::board_id::{BoardIdError, BoardIdSource, Presence, SourceKind, is_sentinel};

/// The customer OTP nvmem device on Raspberry Pi hardware. Readable by root only, which is the
/// privilege the daemon runs under.
pub const DEFAULT_OTP_PATH: &str = "/sys/bus/nvmem/devices/nvmem_cust0/nvmem";

/// Customer-programmable one-time-programmable memory, read through the kernel's nvmem interface.
#[derive(Debug, Clone)]
pub struct OneTimeProgrammableSource {
	path: PathBuf,
}

impl Default for OneTimeProgrammableSource {
	fn default() -> Self {
		Self::new()
	}
}

impl OneTimeProgrammableSource {
	/// Read from the standard customer OTP nvmem device.
	pub fn new() -> Self {
		Self::at(DEFAULT_OTP_PATH)
	}

	/// Read from a given path, for testing against a fixture.
	pub fn at(path: impl Into<PathBuf>) -> Self {
		Self { path: path.into() }
	}

	fn read_raw(&self) -> Result<Option<Vec<u8>>, BoardIdError> {
		match fs::read(&self.path) {
			Ok(raw) if raw.is_empty() => Ok(None),
			Ok(raw) => Ok(Some(raw)),
			Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
			Err(err) => Err(BoardIdError::Backend {
				kind: SourceKind::OneTimeProgrammable,
				message: err.to_string(),
			}),
		}
	}
}

impl BoardIdSource for OneTimeProgrammableSource {
	fn kind(&self) -> SourceKind {
		SourceKind::OneTimeProgrammable
	}

	fn probe(&self) -> Result<Presence, BoardIdError> {
		match self.read_raw()? {
			None => Ok(Presence::Absent),
			// Unwritten memory reads as zeros; an erased region reads as ones.
			Some(raw) if is_sentinel(&raw) => Ok(Presence::Placeholder),
			Some(_) => Ok(Presence::Present),
		}
	}

	fn read(&self) -> Result<Vec<u8>, BoardIdError> {
		self.read_raw()?.ok_or(BoardIdError::Backend {
			kind: SourceKind::OneTimeProgrammable,
			message: "one-time-programmable memory disappeared between probe and read".to_owned(),
		})
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::board_id::backends::tests::Fixture;

	#[test]
	fn unwritten_memory_is_a_placeholder() {
		// The real state of the prototype: 32 bytes of zeros, which must fall through to the serial
		// rather than derive a secret every unprogrammed board would share.
		let f = Fixture::new(&[0u8; 32]);
		let source = OneTimeProgrammableSource::at(f.path());
		assert_eq!(source.probe().unwrap(), Presence::Placeholder);
	}

	#[test]
	fn erased_memory_is_a_placeholder() {
		let f = Fixture::new(&[0xffu8; 32]);
		let source = OneTimeProgrammableSource::at(f.path());
		assert_eq!(source.probe().unwrap(), Presence::Placeholder);
	}

	#[test]
	fn written_memory_is_the_board_id() {
		let mut value = [0u8; 32];
		value[0] = 0xbe;
		value[31] = 0xef;
		let f = Fixture::new(&value);
		let source = OneTimeProgrammableSource::at(f.path());
		assert_eq!(source.probe().unwrap(), Presence::Present);
		assert_eq!(source.read().unwrap(), value.to_vec());
	}

	#[test]
	fn absent_where_the_device_does_not_exist() {
		let source = OneTimeProgrammableSource::at("/nonexistent/bliti/nvmem");
		assert_eq!(source.probe().unwrap(), Presence::Absent);
	}
}
