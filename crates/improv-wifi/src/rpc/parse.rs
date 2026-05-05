use thiserror::Error;
use winnow::{ModalResult, Parser, binary::u8 as parse_u8, error::ContextError, token::take};

use super::{Command, checksum};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ParseError {
	#[error("incomplete RPC packet: need {needed} more bytes")]
	Incomplete { needed: usize },

	#[error("malformed RPC packet")]
	Malformed,

	#[error("RPC packet checksum mismatch")]
	BadChecksum,

	#[error("unknown RPC command id: {0:#04x}")]
	UnknownCommand(u8),

	#[error("RPC payload contains invalid UTF-8")]
	BadUtf8,

	#[error("Send Wi-Fi Settings payload is malformed")]
	BadWifiSettings,
}

/// Outcome of attempting to parse a single packet from the head of a byte buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
	pub command: Command,
	/// Number of bytes consumed from the input.
	pub consumed: usize,
}

/// Parse a single RPC packet from the head of `input`.
///
/// Returns `Ok(Parsed)` on success, [`ParseError::Incomplete`] when the buffer doesn't yet hold a
/// full packet, and any other [`ParseError`] for unrecoverable problems with this packet.
pub fn parse_packet(input: &[u8]) -> Result<Parsed, ParseError> {
	if input.len() < 2 {
		return Err(ParseError::Incomplete {
			needed: 2 - input.len(),
		});
	}

	let command_id = input[0];
	let data_length = input[1] as usize;
	let total_len = 2 + data_length + 1;

	if input.len() < total_len {
		return Err(ParseError::Incomplete {
			needed: total_len - input.len(),
		});
	}

	let header_and_data = &input[..2 + data_length];
	let provided_checksum = input[2 + data_length];
	let expected_checksum = checksum(header_and_data);
	if provided_checksum != expected_checksum {
		return Err(ParseError::BadChecksum);
	}

	let data = &input[2..2 + data_length];
	let command = decode_command(command_id, data)?;
	Ok(Parsed {
		command,
		consumed: total_len,
	})
}

fn decode_command(id: u8, data: &[u8]) -> Result<Command, ParseError> {
	match id {
		0x01 => decode_wifi_settings(data),
		0x02 => expect_empty(data).map(|_| Command::Identify),
		0x03 => expect_empty(data).map(|_| Command::DeviceInfo),
		0x04 => expect_empty(data).map(|_| Command::Scan),
		0x05 => decode_hostname_or_get(data, || Command::GetHostname, Command::SetHostname),
		0x06 => decode_hostname_or_get(data, || Command::GetDeviceName, Command::SetDeviceName),
		other => Err(ParseError::UnknownCommand(other)),
	}
}

fn expect_empty(data: &[u8]) -> Result<(), ParseError> {
	if data.is_empty() {
		Ok(())
	} else {
		Err(ParseError::Malformed)
	}
}

fn decode_hostname_or_get(
	data: &[u8],
	get: impl FnOnce() -> Command,
	set: impl FnOnce(String) -> Command,
) -> Result<Command, ParseError> {
	if data.is_empty() {
		Ok(get())
	} else {
		let s = std::str::from_utf8(data).map_err(|_| ParseError::BadUtf8)?;
		Ok(set(s.to_owned()))
	}
}

fn decode_wifi_settings(data: &[u8]) -> Result<Command, ParseError> {
	let mut input = data;
	let (ssid_bytes, pwd_bytes) = parse_wifi_payload
		.parse_next(&mut input)
		.map_err(|_| ParseError::BadWifiSettings)?;
	if !input.is_empty() {
		return Err(ParseError::BadWifiSettings);
	}
	let ssid = std::str::from_utf8(ssid_bytes)
		.map_err(|_| ParseError::BadUtf8)?
		.to_owned();
	let password = std::str::from_utf8(pwd_bytes)
		.map_err(|_| ParseError::BadUtf8)?
		.to_owned();
	Ok(Command::SendWifiSettings { ssid, password })
}

fn parse_wifi_payload<'a>(input: &mut &'a [u8]) -> ModalResult<(&'a [u8], &'a [u8]), ContextError> {
	let ssid_len = parse_u8.parse_next(input)?;
	let ssid_bytes = take(ssid_len as usize).parse_next(input)?;
	let pwd_len = parse_u8.parse_next(input)?;
	let pwd_bytes = take(pwd_len as usize).parse_next(input)?;
	Ok((ssid_bytes, pwd_bytes))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn build(command_id: u8, data: &[u8]) -> Vec<u8> {
		let mut packet = Vec::with_capacity(2 + data.len() + 1);
		packet.push(command_id);
		packet.push(data.len() as u8);
		packet.extend_from_slice(data);
		let cs = checksum(&packet);
		packet.push(cs);
		packet
	}

	#[test]
	fn empty_buffer_is_incomplete() {
		assert_eq!(parse_packet(&[]), Err(ParseError::Incomplete { needed: 2 }));
		assert_eq!(
			parse_packet(&[0x02]),
			Err(ParseError::Incomplete { needed: 1 })
		);
	}

	#[test]
	fn header_only_is_incomplete() {
		// Header says data_length=4 but no data or checksum.
		assert_eq!(
			parse_packet(&[0x01, 0x04]),
			Err(ParseError::Incomplete { needed: 5 })
		);
	}

	#[test]
	fn bad_checksum_is_rejected() {
		let mut pkt = build(0x02, &[]);
		let last = pkt.len() - 1;
		pkt[last] = pkt[last].wrapping_add(1);
		assert_eq!(parse_packet(&pkt), Err(ParseError::BadChecksum));
	}

	#[test]
	fn identify_round_trip() {
		let pkt = build(0x02, &[]);
		let parsed = parse_packet(&pkt).unwrap();
		assert_eq!(parsed.command, Command::Identify);
		assert_eq!(parsed.consumed, pkt.len());
	}

	#[test]
	fn device_info_round_trip() {
		let pkt = build(0x03, &[]);
		assert_eq!(parse_packet(&pkt).unwrap().command, Command::DeviceInfo);
	}

	#[test]
	fn scan_round_trip() {
		let pkt = build(0x04, &[]);
		assert_eq!(parse_packet(&pkt).unwrap().command, Command::Scan);
	}

	#[test]
	fn wifi_settings_round_trip() {
		let mut data = Vec::new();
		data.push(4); // ssid len
		data.extend_from_slice(b"home");
		data.push(8); // pwd len
		data.extend_from_slice(b"abcd1234");
		let pkt = build(0x01, &data);
		let parsed = parse_packet(&pkt).unwrap();
		assert_eq!(
			parsed.command,
			Command::SendWifiSettings {
				ssid: "home".into(),
				password: "abcd1234".into(),
			}
		);
	}

	#[test]
	fn wifi_settings_empty_password() {
		let mut data = Vec::new();
		data.push(4);
		data.extend_from_slice(b"open");
		data.push(0);
		let pkt = build(0x01, &data);
		assert_eq!(
			parse_packet(&pkt).unwrap().command,
			Command::SendWifiSettings {
				ssid: "open".into(),
				password: String::new(),
			}
		);
	}

	#[test]
	fn wifi_settings_truncated_payload() {
		let mut data = Vec::new();
		data.push(4);
		data.extend_from_slice(b"home");
		// Missing password length.
		let pkt = build(0x01, &data);
		assert_eq!(parse_packet(&pkt), Err(ParseError::BadWifiSettings));
	}

	#[test]
	fn wifi_settings_extra_trailing_bytes() {
		let mut data = Vec::new();
		data.push(2);
		data.extend_from_slice(b"hi");
		data.push(2);
		data.extend_from_slice(b"pw");
		data.push(0xff); // junk
		let pkt = build(0x01, &data);
		assert_eq!(parse_packet(&pkt), Err(ParseError::BadWifiSettings));
	}

	#[test]
	fn hostname_get_vs_set() {
		assert_eq!(
			parse_packet(&build(0x05, &[])).unwrap().command,
			Command::GetHostname
		);
		assert_eq!(
			parse_packet(&build(0x05, b"my-host")).unwrap().command,
			Command::SetHostname("my-host".into())
		);
	}

	#[test]
	fn device_name_get_vs_set() {
		assert_eq!(
			parse_packet(&build(0x06, &[])).unwrap().command,
			Command::GetDeviceName
		);
		assert_eq!(
			parse_packet(&build(0x06, "Frosty's Device".as_bytes()))
				.unwrap()
				.command,
			Command::SetDeviceName("Frosty's Device".into())
		);
	}

	#[test]
	fn unknown_command_id() {
		let pkt = build(0x99, &[]);
		assert_eq!(parse_packet(&pkt), Err(ParseError::UnknownCommand(0x99)));
	}

	#[test]
	fn identify_with_unexpected_data_is_malformed() {
		let pkt = build(0x02, &[0xab]);
		assert_eq!(parse_packet(&pkt), Err(ParseError::Malformed));
	}

	#[test]
	fn invalid_utf8_in_hostname() {
		let pkt = build(0x05, &[0xff, 0xfe, 0xfd]);
		assert_eq!(parse_packet(&pkt), Err(ParseError::BadUtf8));
	}

	#[test]
	fn extra_bytes_after_packet_are_left_for_caller() {
		let mut buf = build(0x02, &[]);
		buf.extend_from_slice(&[0xaa, 0xbb]);
		let parsed = parse_packet(&buf).unwrap();
		assert_eq!(parsed.command, Command::Identify);
		assert_eq!(parsed.consumed, buf.len() - 2);
	}
}
