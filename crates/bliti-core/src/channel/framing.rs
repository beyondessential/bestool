//! Framing and reassembly directly above GATT.
//!
//! Behaviour is specified in BLI-CHN, "Transport". GATT carries reliable, ordered bytes, but a
//! client writes and a device notifies in chunks no larger than the negotiated attribute size, and a
//! message may span several. Framing prefixes each message with its length so the far end can
//! reassemble it across the chunks, so a message is not limited by the attribute size.
//!
//! Each framed message is one Noise message — a handshake message or a transport message — so the
//! default maximum here is the largest a Noise message can be. A peer that claims a larger length is
//! refused rather than allowed to make the far end buffer without bound.

use super::ChannelError;

/// The length prefix width: a four-byte big-endian message length.
const LENGTH_PREFIX: usize = 4;

/// The default largest message the reassembler will buffer: the largest a Noise message can be.
pub const DEFAULT_MAX_MESSAGE: usize = 65535;

/// Frame a message for sending: a four-byte big-endian length followed by the message bytes.
pub fn frame(message: &[u8]) -> Vec<u8> {
	let mut framed = Vec::with_capacity(LENGTH_PREFIX + message.len());
	framed.extend_from_slice(&(message.len() as u32).to_be_bytes());
	framed.extend_from_slice(message);
	framed
}

/// Reassembles framed messages from the chunks a transport delivers.
///
/// Chunks are pushed in as they arrive, in any sizes, and complete messages are taken out as they
/// become available. A claimed length beyond the maximum is refused.
#[derive(Debug)]
pub struct Reassembler {
	buf: Vec<u8>,
	max: usize,
}

impl Default for Reassembler {
	fn default() -> Self {
		Self::new()
	}
}

impl Reassembler {
	/// A reassembler buffering messages up to [`DEFAULT_MAX_MESSAGE`].
	pub fn new() -> Self {
		Self::with_max(DEFAULT_MAX_MESSAGE)
	}

	/// A reassembler buffering messages up to `max` bytes.
	pub fn with_max(max: usize) -> Self {
		Self {
			buf: Vec::new(),
			max,
		}
	}

	/// Add a chunk as delivered by the transport.
	pub fn push(&mut self, chunk: &[u8]) {
		self.buf.extend_from_slice(chunk);
	}

	/// Take the next complete message, if one is available. Returns `Ok(None)` when more bytes are
	/// needed, and an error when a peer claims a length beyond the maximum.
	pub fn take(&mut self) -> Result<Option<Vec<u8>>, ChannelError> {
		if self.buf.len() < LENGTH_PREFIX {
			return Ok(None);
		}
		let claimed =
			u32::from_be_bytes(self.buf[..LENGTH_PREFIX].try_into().expect("four bytes")) as usize;
		if claimed > self.max {
			return Err(ChannelError::FrameTooLarge {
				claimed,
				max: self.max,
			});
		}
		if self.buf.len() < LENGTH_PREFIX + claimed {
			return Ok(None);
		}
		let message = self.buf[LENGTH_PREFIX..LENGTH_PREFIX + claimed].to_vec();
		self.buf.drain(..LENGTH_PREFIX + claimed);
		Ok(Some(message))
	}

	/// Push a chunk and take every message it completes.
	pub fn push_and_drain(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, ChannelError> {
		self.push(chunk);
		let mut messages = Vec::new();
		while let Some(message) = self.take()? {
			messages.push(message);
		}
		Ok(messages)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn frame_prefixes_the_length() {
		assert_eq!(frame(b"hi"), vec![0, 0, 0, 2, b'h', b'i']);
		assert_eq!(frame(b""), vec![0, 0, 0, 0]);
	}

	#[test]
	fn reassembles_a_message_delivered_in_one_chunk() {
		let mut r = Reassembler::new();
		let messages = r.push_and_drain(&frame(b"hello")).unwrap();
		assert_eq!(messages, vec![b"hello".to_vec()]);
	}

	#[test]
	fn reassembles_a_message_split_across_chunks() {
		let mut r = Reassembler::new();
		let framed = frame(b"a longer message than one chunk");
		// Deliver a byte at a time; only the final byte completes the message.
		for (i, byte) in framed.iter().enumerate() {
			let out = r.push_and_drain(&[*byte]).unwrap();
			if i + 1 < framed.len() {
				assert!(out.is_empty());
			} else {
				assert_eq!(out, vec![b"a longer message than one chunk".to_vec()]);
			}
		}
	}

	#[test]
	fn separates_several_messages_in_one_chunk() {
		let mut r = Reassembler::new();
		let mut chunk = frame(b"one");
		chunk.extend(frame(b"two"));
		chunk.extend(frame(b"three"));
		let messages = r.push_and_drain(&chunk).unwrap();
		assert_eq!(
			messages,
			vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
		);
	}

	#[test]
	fn holds_a_partial_trailing_message() {
		let mut r = Reassembler::new();
		let mut chunk = frame(b"complete");
		chunk.extend_from_slice(&[0, 0, 0, 10, b'p', b'a', b'r', b't']); // header + 4 of 10 bytes
		let messages = r.push_and_drain(&chunk).unwrap();
		assert_eq!(messages, vec![b"complete".to_vec()]);
		// The rest of the partial message completes it later.
		let messages = r.push_and_drain(b"ial!!!").unwrap();
		assert_eq!(messages, vec![b"partial!!!".to_vec()]);
	}

	#[test]
	fn refuses_an_over_large_claimed_length() {
		let mut r = Reassembler::with_max(16);
		let mut chunk = 20u32.to_be_bytes().to_vec();
		chunk.extend_from_slice(&[0u8; 20]);
		assert!(matches!(
			r.push_and_drain(&chunk),
			Err(ChannelError::FrameTooLarge {
				claimed: 20,
				max: 16
			})
		));
	}
}
