use thiserror::Error;

/// Improv Wi-Fi error codes, as transmitted on the Error State characteristic.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Error {
	/// RPC packet was malformed or had a bad checksum.
	#[error("invalid RPC packet")]
	InvalidRPC = 0x01,

	/// The command sent is unknown.
	#[error("unknown RPC command")]
	UnknownRPC = 0x02,

	/// Credentials were received but the device couldn't connect to the network.
	#[error("unable to connect to the requested network")]
	UnableToConnect = 0x03,

	/// Credentials were sent via RPC but the Improv service is not authorised.
	#[error("not authorised")]
	NotAuthorized = 0x04,

	/// A hostname value is not RFC 1123 compliant.
	#[error("bad hostname")]
	BadHostname = 0x05,

	/// Catch-all for backend errors.
	#[error("unknown error")]
	Unknown = 0xFF,
}

impl Error {
	pub fn as_byte(self) -> u8 {
		self as u8
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn error_byte_values() {
		assert_eq!(Error::InvalidRPC.as_byte(), 0x01);
		assert_eq!(Error::UnknownRPC.as_byte(), 0x02);
		assert_eq!(Error::UnableToConnect.as_byte(), 0x03);
		assert_eq!(Error::NotAuthorized.as_byte(), 0x04);
		assert_eq!(Error::BadHostname.as_byte(), 0x05);
		assert_eq!(Error::Unknown.as_byte(), 0xFF);
	}
}
