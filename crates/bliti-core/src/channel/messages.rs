//! The application messages carried over the channel, as JSON.
//!
//! Behaviour is specified in BLI-CHN, "Messages". The volumes are small, every client platform reads
//! JSON without a library, and a conversation can be read directly while developing. A device that
//! receives a message it does not understand says so, rather than closing the channel.
//!
//! Milestone one carries two things, one in each direction: a line of text from the client that the
//! device prints, proving the client-to-device direction, and the device's hostname and addresses,
//! proving the other. The message types are transport-agnostic; they ride on a stream once the
//! stream layer is wired in.

use serde::{Deserialize, Serialize};

/// A message from the client to the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClientMessage {
	/// A line of text for the device to print on its standard output, proving the client-to-device
	/// direction (BLI-CHN; channel demonstration).
	Text {
		/// The text to print.
		text: String,
	},
}

/// A message from the device to the client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum DeviceMessage {
	/// The device's identity: its hostname and network addresses. Sent when a client connects and
	/// again whenever it changes, unsolicited, standing in for the Iti's LCD.
	Identity {
		/// The device's hostname.
		hostname: String,
		/// Every global address the device has, each with the interface it belongs to. Loopback and
		/// link-local addresses are left out; the client decides what is worth showing.
		addresses: Vec<Address>,
	},

	/// A message the device could not understand, reported rather than closing the channel.
	Unknown {
		/// Why the message could not be understood.
		reason: String,
	},
}

/// One network address the device holds, with the interface it belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
	/// The address in its textual form.
	pub address: String,
	/// The interface the address belongs to.
	pub interface: String,
	/// The address family.
	pub family: AddressFamily,
}

/// The family of a network address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddressFamily {
	/// An IPv4 address.
	Ipv4,
	/// An IPv6 address.
	Ipv6,
}

impl ClientMessage {
	/// Serialise to JSON bytes.
	pub fn to_json(&self) -> Vec<u8> {
		serde_json::to_vec(self).expect("client message serialises")
	}
}

impl DeviceMessage {
	/// Serialise to JSON bytes.
	pub fn to_json(&self) -> Vec<u8> {
		serde_json::to_vec(self).expect("device message serialises")
	}
}

/// Parse a message from a client, on the device side.
///
/// On success the parsed message is returned. On failure, rather than an error that would tempt the
/// caller to close the channel, a [`DeviceMessage::Unknown`] is returned for the device to send back:
/// a message it does not understand is reported, and the channel stays open (BLI-CHN, "Messages").
pub fn parse_client_message(bytes: &[u8]) -> Result<ClientMessage, DeviceMessage> {
	serde_json::from_slice(bytes).map_err(|err| DeviceMessage::Unknown {
		reason: err.to_string(),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn client_text_round_trips() {
		let message = ClientMessage::Text {
			text: "hello from the browser".to_owned(),
		};
		let json = message.to_json();
		assert_eq!(parse_client_message(&json).unwrap(), message);
	}

	#[test]
	fn client_text_json_shape_is_stable() {
		let json = ClientMessage::Text {
			text: "hi".to_owned(),
		}
		.to_json();
		assert_eq!(
			String::from_utf8(json).unwrap(),
			r#"{"type":"text","text":"hi"}"#
		);
	}

	#[test]
	fn device_identity_round_trips() {
		let message = DeviceMessage::Identity {
			hostname: "tamanu-iti".to_owned(),
			addresses: vec![
				Address {
					address: "192.0.2.10".to_owned(),
					interface: "eth0".to_owned(),
					family: AddressFamily::Ipv4,
				},
				Address {
					address: "2001:db8::1".to_owned(),
					interface: "wlan0".to_owned(),
					family: AddressFamily::Ipv6,
				},
			],
		};
		let json = message.to_json();
		let back: DeviceMessage = serde_json::from_slice(&json).unwrap();
		assert_eq!(back, message);
	}

	#[test]
	fn an_unparseable_message_yields_unknown_not_an_error() {
		// A device replies that it did not understand, rather than closing the channel.
		let response = parse_client_message(b"this is not json").unwrap_err();
		assert!(matches!(response, DeviceMessage::Unknown { .. }));
	}

	#[test]
	fn an_unknown_message_type_yields_unknown() {
		let response = parse_client_message(br#"{"type":"reboot"}"#).unwrap_err();
		assert!(matches!(response, DeviceMessage::Unknown { .. }));
	}
}
