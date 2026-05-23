//! Improv-Wi-Fi RPC packet types and parsing.
//!
//! Packets sent by the client on the RPC Command characteristic have the structure:
//!
//! ```text
//! [command_id: u8] [data_length: u8] [data: u8 * data_length] [checksum: u8]
//! ```
//!
//! Where `checksum` is the least-significant byte of the additive sum of all preceding bytes
//! (including `command_id` and `data_length`).

mod parse;
mod reassembly;
mod result;

pub use parse::{ParseError, parse_packet};
pub use reassembly::{Reassembler, Yielded};
pub use result::encode_response;

/// A parsed RPC command from the client.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Command {
	/// `0x01` Send Wi-Fi Settings — provision the device with the given credentials.
	SendWifiSettings { ssid: String, password: String },

	/// `0x02` Identify — make the device perform a visual or audible signal.
	Identify,

	/// `0x03` Device Info — request the device's firmware/hardware info.
	DeviceInfo,

	/// `0x04` Scan — list visible Wi-Fi networks.
	Scan,

	/// `0x05` (zero data) Get the device hostname.
	GetHostname,

	/// `0x05` (with data) Set the device hostname.
	SetHostname(String),

	/// `0x06` (zero data) Get the device name.
	GetDeviceName,

	/// `0x06` (with data) Set the device name.
	SetDeviceName(String),
}

impl Command {
	/// The command ID byte that introduces this command on the wire.
	pub fn id(&self) -> u8 {
		match self {
			Self::SendWifiSettings { .. } => 0x01,
			Self::Identify => 0x02,
			Self::DeviceInfo => 0x03,
			Self::Scan => 0x04,
			Self::GetHostname | Self::SetHostname(_) => 0x05,
			Self::GetDeviceName | Self::SetDeviceName(_) => 0x06,
		}
	}
}

/// Compute the LSB-only additive checksum used by Improv-Wi-Fi packets.
pub fn checksum(bytes: &[u8]) -> u8 {
	bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn checksum_lsb_wraps() {
		assert_eq!(checksum(&[]), 0);
		assert_eq!(checksum(&[0x01, 0x02, 0x03]), 0x06);
		// 0xFF + 0x01 wraps to 0x00.
		assert_eq!(checksum(&[0xFF, 0x01]), 0x00);
		// Sum of 256 ones = 256 → LSB 0.
		assert_eq!(checksum(&[1u8; 256]), 0);
	}
}
