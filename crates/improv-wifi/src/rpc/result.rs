use super::checksum;

/// Encode an RPC Result packet for the given command id and string list.
///
/// Wire format:
///
/// ```text
/// [command_id] [data_length] [s1_len] [s1] [s2_len] [s2] ... [checksum]
/// ```
///
/// `data_length` is the total payload length (sum of `1 + s.len()` for each string).
pub fn encode_response(command_id: u8, strings: &[impl AsRef<[u8]>]) -> Vec<u8> {
	let mut data: Vec<u8> = Vec::new();
	for s in strings {
		let bytes = s.as_ref();
		assert!(bytes.len() <= u8::MAX as usize, "RPC string too long");
		data.push(bytes.len() as u8);
		data.extend_from_slice(bytes);
	}
	assert!(data.len() <= u8::MAX as usize, "RPC payload too long");

	let mut packet = Vec::with_capacity(2 + data.len() + 1);
	packet.push(command_id);
	packet.push(data.len() as u8);
	packet.extend_from_slice(&data);
	let cs = checksum(&packet);
	packet.push(cs);
	packet
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn empty_response() {
		let pkt = encode_response(0x05, &[] as &[&str]);
		assert_eq!(pkt, vec![0x05, 0x00, 0x05]);
	}

	#[test]
	fn single_string_response() {
		let pkt = encode_response(0x05, &["hi"]);
		// 0x05, 0x03, 0x02, 'h', 'i', cs
		// cs = 5 + 3 + 2 + 104 + 105 = 219 = 0xDB
		assert_eq!(pkt, vec![0x05, 0x03, 0x02, b'h', b'i', 0xDB]);
	}

	#[test]
	fn multi_string_response() {
		let pkt = encode_response(0x01, &["http://x", "extra"]);
		// data = [8, "http://x", 5, "extra"] -> 1+8+1+5 = 15 bytes
		assert_eq!(pkt[0], 0x01);
		assert_eq!(pkt[1], 15);
		// payload starts at index 2
		assert_eq!(&pkt[2..3], &[8u8]);
		assert_eq!(&pkt[3..11], b"http://x");
		assert_eq!(&pkt[11..12], &[5u8]);
		assert_eq!(&pkt[12..17], b"extra");
		// last byte is checksum over pkt[..17]
		let cs = checksum(&pkt[..pkt.len() - 1]);
		assert_eq!(*pkt.last().unwrap(), cs);
	}
}
