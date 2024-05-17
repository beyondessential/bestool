#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Error {
	/// RPC packet was malformed/invalid.
	InvalidRPC = 0x01,

	/// The command sent is unknown.
	UnknownRPC = 0x02,

	/// The credentials have been received and an attempt to connect to the network has failed.
	UnableToConnect = 0x03,

	/// Credentials were sent via RPC but the Improv service is not authorized.
	NotAuthorized = 0x04,

	/// Unknown error.
	Unknown = 0xFF,
}

impl Error {
	pub fn as_byte(&self) -> u8 {
		*self as _
	}
}
