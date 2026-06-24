//! Streaming re-encode of minio-go's chunked PUT bodies.
//!
//! A streaming upload arrives as `aws-chunked` framing: a sequence of
//! `<hexsize>;chunk-signature=<64hex>\r\n<data>\r\n` chunks ending in a
//! zero-length chunk. The in-body signatures were computed with kopia's dummy
//! key, so they would not verify upstream; [`ChunkResigner`] reads each chunk
//! and re-emits it with a signature recomputed from the live signing key,
//! chained from the request's seed signature.
//!
//! The chunk sizes are copied verbatim, so the re-encoded body is byte-for-byte
//! the same length and the request's `Content-Length` is unchanged. Only the
//! current chunk is held in memory (minio-go's default chunk is 64 KiB), so an
//! arbitrarily large object streams through without being buffered whole.

use super::sigv4;

/// Largest single chunk payload the proxy will buffer and re-sign. Far above the
/// 64 KiB chunks kopia emits; a larger chunk is rejected rather than buffered
/// without bound, so peak memory per in-flight request stays bounded.
pub const MAX_CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// Largest chunk header line (`<hexsize>;chunk-signature=<64hex>`) accepted
/// before its terminating CRLF — guards against an unterminated header growing
/// the buffer without bound.
const MAX_HEADER_LEN: usize = 1024;

/// Malformed or oversized `aws-chunked` framing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkError {
	/// A chunk header's size field was not valid hex.
	BadSize,
	/// A chunk declared a payload larger than [`MAX_CHUNK_SIZE`].
	ChunkTooLarge { size: usize },
	/// A chunk header line ran past [`MAX_HEADER_LEN`] without terminating.
	HeaderTooLong,
}

impl std::fmt::Display for ChunkError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			ChunkError::BadSize => f.write_str("invalid chunk size in aws-chunked framing"),
			ChunkError::ChunkTooLarge { size } => write!(
				f,
				"aws-chunked chunk of {size} bytes exceeds the {MAX_CHUNK_SIZE}-byte limit"
			),
			ChunkError::HeaderTooLong => write!(
				f,
				"aws-chunked chunk header exceeded {MAX_HEADER_LEN} bytes without terminating"
			),
		}
	}
}

impl std::error::Error for ChunkError {}

/// Re-signs an `aws-chunked` request body chunk by chunk as bytes arrive.
pub struct ChunkResigner {
	signing_key: Vec<u8>,
	amz_date: String,
	scope: String,
	prev_sig: String,
	buf: Vec<u8>,
	finished: bool,
}

impl ChunkResigner {
	/// `seed_sig` is the re-signed request's seed signature (the `Signature=` in
	/// its `Authorization` header); it seeds the first chunk's chain.
	pub fn new(signing_key: Vec<u8>, amz_date: String, scope: String, seed_sig: String) -> Self {
		Self {
			signing_key,
			amz_date,
			scope,
			prev_sig: seed_sig,
			buf: Vec::new(),
			finished: false,
		}
	}

	/// True once the terminating zero-length chunk has been emitted.
	pub fn finished(&self) -> bool {
		self.finished
	}

	/// Feed freshly-read body bytes; returns the re-signed bytes ready to
	/// forward upstream (every complete chunk now available). Bytes for a chunk
	/// not yet wholly received are retained until the rest arrives.
	pub fn push(&mut self, input: &[u8]) -> Result<Vec<u8>, ChunkError> {
		self.buf.extend_from_slice(input);
		let mut out = Vec::new();
		let mut cursor = 0;
		loop {
			let Some(rel) = find(&self.buf[cursor..], b"\r\n") else {
				// No complete header line yet; reject an unterminated one.
				if self.buf.len().saturating_sub(cursor) > MAX_HEADER_LEN {
					return Err(ChunkError::HeaderTooLong);
				}
				break;
			};
			let header_end = cursor + rel;
			let line = &self.buf[cursor..header_end];
			let hexsize = line.split(|&b| b == b';').next().unwrap_or(line);
			let size = parse_hex(hexsize).ok_or(ChunkError::BadSize)?;
			if size > MAX_CHUNK_SIZE {
				return Err(ChunkError::ChunkTooLarge { size });
			}
			let data_start = header_end + 2;
			// header line + CRLF + data + trailing CRLF (size is bounded above,
			// so this cannot overflow)
			let chunk_end = data_start + size + 2;
			if self.buf.len() < chunk_end {
				break; // chunk not fully received yet
			}
			let data = &self.buf[data_start..data_start + size];
			let new_sig = sigv4::chunk_signature(
				&self.signing_key,
				&self.amz_date,
				&self.scope,
				&self.prev_sig,
				data,
			);
			out.extend_from_slice(hexsize);
			out.extend_from_slice(b";chunk-signature=");
			out.extend_from_slice(new_sig.as_bytes());
			out.extend_from_slice(b"\r\n");
			out.extend_from_slice(data);
			out.extend_from_slice(b"\r\n");
			self.prev_sig = new_sig;
			cursor = chunk_end;
			if size == 0 {
				self.finished = true;
				break;
			}
		}
		self.buf.drain(..cursor);
		Ok(out)
	}
}

fn parse_hex(bytes: &[u8]) -> Option<usize> {
	let s = std::str::from_utf8(bytes).ok()?.trim();
	usize::from_str_radix(s, 16).ok()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
	haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
	use super::*;

	const S3_SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
	const S3_AMZ_DATE: &str = "20130524T000000Z";
	const S3_SEED: &str = "4f232c4386841ef735655705268965c44a0e4690baa4adea153f7db9fa80a0a9";
	const CHUNK1_SIG: &str = "ad80c730a21e5b8d04586a2213dd63b9a0e99e0e2307b0ade35a65485a288648";
	const CHUNK2_SIG: &str = "0055627c9e194cb4542bae2aa5492e3c1575bbb81b612b7d234b86a503ef5497";
	const LAST_SIG: &str = "b6c6ea8a5354eaf15b3cb7646744f4275b71ea724fed81ceb9323e279d449df9";

	fn framed(hexsize: &str, sig: &str, data: &[u8]) -> Vec<u8> {
		let mut v = Vec::new();
		v.extend_from_slice(hexsize.as_bytes());
		v.extend_from_slice(b";chunk-signature=");
		v.extend_from_slice(sig.as_bytes());
		v.extend_from_slice(b"\r\n");
		v.extend_from_slice(data);
		v.extend_from_slice(b"\r\n");
		v
	}

	fn resigner() -> ChunkResigner {
		let key = sigv4::signing_key(S3_SECRET, "20130524", "us-east-1", "s3");
		ChunkResigner::new(
			key,
			S3_AMZ_DATE.into(),
			sigv4::scope("20130524", "us-east-1", "s3"),
			S3_SEED.into(),
		)
	}

	// Input body with placeholder (dummy-key) signatures; output must carry the
	// signatures from AWS's documented multi-chunk streaming-PUT example.
	fn dummy_input() -> Vec<u8> {
		let placeholder = "0".repeat(64);
		let mut input = framed("10000", &placeholder, &[b'a'; 65536]);
		input.extend(framed("400", &placeholder, &[b'a'; 1024]));
		input.extend(framed("0", &placeholder, b""));
		input
	}

	fn expected_output() -> Vec<u8> {
		let mut out = framed("10000", CHUNK1_SIG, &[b'a'; 65536]);
		out.extend(framed("400", CHUNK2_SIG, &[b'a'; 1024]));
		out.extend(framed("0", LAST_SIG, b""));
		out
	}

	#[test]
	fn resigns_whole_body_to_aws_vector() {
		let mut r = resigner();
		let out = r.push(&dummy_input()).unwrap();
		assert!(r.finished());
		assert_eq!(out, expected_output());
		// Framing preserved: re-encoded body is the same length as the input.
		assert_eq!(out.len(), dummy_input().len());
	}

	#[test]
	fn streaming_in_fragments_is_identical() {
		// Feed the body in awkward 7-byte fragments straddling chunk boundaries.
		let input = dummy_input();
		let mut r = resigner();
		let mut out = Vec::new();
		for frag in input.chunks(7) {
			out.extend(r.push(frag).unwrap());
		}
		assert!(r.finished());
		assert_eq!(out, expected_output());
	}

	#[test]
	fn bad_hex_size_errors() {
		let mut r = resigner();
		let err = r.push(b"zzzz;chunk-signature=x\r\ndata\r\n").unwrap_err();
		assert_eq!(err, ChunkError::BadSize);
	}

	#[test]
	fn chunk_larger_than_limit_errors() {
		let mut r = resigner();
		// Declare 32 MiB (0x2000000), over the 8 MiB cap; rejected on the header
		// alone, before any payload is buffered.
		let header = format!("2000000;chunk-signature={}\r\n", "0".repeat(64));
		let err = r.push(header.as_bytes()).unwrap_err();
		assert_eq!(err, ChunkError::ChunkTooLarge { size: 0x2000000 });
	}

	#[test]
	fn unterminated_header_errors() {
		let mut r = resigner();
		let err = r.push(&vec![b'a'; MAX_HEADER_LEN + 1]).unwrap_err();
		assert_eq!(err, ChunkError::HeaderTooLong);
	}
}
