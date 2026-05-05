use super::{
	Command,
	parse::{ParseError, parse_packet},
};

/// Buffers raw bytes from successive BLE writes and yields complete RPC packets.
///
/// BLE writes carry up to one MTU (often ~20 bytes by default) per attribute write, but Improv-Wi-Fi
/// payloads can exceed that, so a client may split one logical packet across several writes. This
/// reassembler accumulates bytes until a full packet can be parsed, returning each as soon as it
/// becomes available.
#[derive(Debug, Default)]
pub struct Reassembler {
	buf: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Yielded {
	/// A full packet was decoded.
	Command(Command),
	/// The current packet was malformed; the buffer has been cleared so the next bytes start a
	/// fresh packet. Inspect the inner [`ParseError`] to surface to the client.
	Error(ParseError),
}

impl Reassembler {
	pub fn new() -> Self {
		Self::default()
	}

	/// Append bytes from a fresh BLE write and drain all complete packets.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<Yielded> {
		self.buf.extend_from_slice(bytes);
		let mut out = Vec::new();
		loop {
			match parse_packet(&self.buf) {
				Ok(parsed) => {
					self.buf.drain(..parsed.consumed);
					out.push(Yielded::Command(parsed.command));
				}
				Err(ParseError::Incomplete { .. }) => break,
				Err(other) => {
					self.buf.clear();
					out.push(Yielded::Error(other));
					break;
				}
			}
		}
		out
	}

	#[cfg(test)]
	fn buffered(&self) -> &[u8] {
		&self.buf
	}
}

#[cfg(test)]
mod tests {
	use super::super::checksum;
	use super::*;

	fn build(command_id: u8, data: &[u8]) -> Vec<u8> {
		let mut p = Vec::with_capacity(2 + data.len() + 1);
		p.push(command_id);
		p.push(data.len() as u8);
		p.extend_from_slice(data);
		let cs = checksum(&p);
		p.push(cs);
		p
	}

	#[test]
	fn single_packet_one_chunk() {
		let mut r = Reassembler::new();
		let pkt = build(0x02, &[]);
		let out = r.feed(&pkt);
		assert_eq!(out, vec![Yielded::Command(Command::Identify)]);
		assert!(r.buffered().is_empty());
	}

	#[test]
	fn split_mid_header() {
		let mut r = Reassembler::new();
		let pkt = build(0x02, &[]);
		assert!(r.feed(&pkt[..1]).is_empty());
		let out = r.feed(&pkt[1..]);
		assert_eq!(out, vec![Yielded::Command(Command::Identify)]);
	}

	#[test]
	fn split_mid_payload() {
		let mut payload = Vec::new();
		payload.push(4);
		payload.extend_from_slice(b"home");
		payload.push(4);
		payload.extend_from_slice(b"abcd");
		let pkt = build(0x01, &payload);

		let mut r = Reassembler::new();
		// Feed in tiny chunks to exercise reassembly across many writes.
		for chunk in pkt.chunks(3) {
			let out = r.feed(chunk);
			if !out.is_empty() {
				assert_eq!(
					out,
					vec![Yielded::Command(Command::SendWifiSettings {
						ssid: "home".into(),
						password: "abcd".into()
					})]
				);
				assert!(r.buffered().is_empty());
				return;
			}
		}
		panic!("never yielded a complete packet");
	}

	#[test]
	fn two_packets_in_one_chunk() {
		let mut r = Reassembler::new();
		let mut combined = build(0x02, &[]);
		combined.extend_from_slice(&build(0x03, &[]));
		let out = r.feed(&combined);
		assert_eq!(
			out,
			vec![
				Yielded::Command(Command::Identify),
				Yielded::Command(Command::DeviceInfo),
			]
		);
	}

	#[test]
	fn bad_checksum_clears_buffer() {
		let mut r = Reassembler::new();
		let mut pkt = build(0x02, &[]);
		let last = pkt.len() - 1;
		pkt[last] = pkt[last].wrapping_add(1);
		let out = r.feed(&pkt);
		assert_eq!(out, vec![Yielded::Error(ParseError::BadChecksum)]);
		assert!(r.buffered().is_empty());
	}

	#[test]
	fn partial_packet_held_across_calls() {
		let mut r = Reassembler::new();
		let pkt = build(0x05, b"hello");
		// Feed one byte at a time.
		let mut yielded = None;
		for (i, b) in pkt.iter().enumerate() {
			let out = r.feed(std::slice::from_ref(b));
			if i + 1 < pkt.len() {
				assert!(out.is_empty(), "yielded too early at byte {i}");
			} else {
				yielded = Some(out);
			}
		}
		assert_eq!(
			yielded.unwrap(),
			vec![Yielded::Command(Command::SetHostname("hello".into()))]
		);
	}
}
