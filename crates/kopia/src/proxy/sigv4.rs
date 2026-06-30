//! SigV4 signing primitives for the re-signing proxy.
//!
//! Just enough of AWS Signature Version 4 to re-sign kopia's S3 requests: the
//! signing-key derivation, the canonical request and string-to-sign, and the
//! per-chunk signature chain that minio-go's streaming PUT bodies
//! (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`) require. Locked to the published AWS
//! test vectors in the tests below.

use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// SHA-256 of the empty string, hex-encoded — the payload-hash slot in every
/// chunk's string-to-sign.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// The `x-amz-content-sha256` value minio-go sends for a chunked streaming PUT.
pub const STREAMING_PAYLOAD: &str = "STREAMING-AWS4-HMAC-SHA256-PAYLOAD";

/// HMAC-SHA256(key, msg).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
	let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
	mac.update(msg);
	mac.finalize().into_bytes().to_vec()
}

/// SHA-256(data), hex-encoded.
pub fn sha256_hex(data: &[u8]) -> String {
	hex::encode(Sha256::digest(data))
}

/// Derive the SigV4 signing key: the four-step HMAC chain over the secret,
/// date, region and service.
pub fn signing_key(secret: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
	let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date_stamp.as_bytes());
	let k_region = hmac_sha256(&k_date, region.as_bytes());
	let k_service = hmac_sha256(&k_region, service.as_bytes());
	hmac_sha256(&k_service, b"aws4_request")
}

/// The credential scope: `<date_stamp>/<region>/<service>/aws4_request`.
pub fn scope(date_stamp: &str, region: &str, service: &str) -> String {
	format!("{date_stamp}/{region}/{service}/aws4_request")
}

/// Assemble the canonical request from its already-canonicalised parts.
pub fn canonical_request(
	method: &str,
	canonical_uri: &str,
	canonical_query: &str,
	canonical_headers: &str,
	signed_headers: &str,
	hashed_payload: &str,
) -> String {
	format!(
		"{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{hashed_payload}"
	)
}

/// `AWS4-HMAC-SHA256\n<amz_date>\n<scope>\n<sha256_hex(canonical_request)>`.
pub fn string_to_sign(amz_date: &str, scope: &str, canonical_request: &str) -> String {
	format!(
		"AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
		sha256_hex(canonical_request.as_bytes())
	)
}

/// The request (seed) signature: HMAC-SHA256(signing_key, string_to_sign), hex.
pub fn signature(signing_key: &[u8], string_to_sign: &str) -> String {
	hex::encode(hmac_sha256(signing_key, string_to_sign.as_bytes()))
}

/// One chunk's signature in the streaming chain. `prev` is the previous
/// signature — the request seed signature for the first chunk, then each
/// chunk's own signature thereafter, through to the terminating zero-length
/// chunk.
pub fn chunk_signature(
	signing_key: &[u8],
	amz_date: &str,
	scope: &str,
	prev: &str,
	chunk_data: &[u8],
) -> String {
	let sts = format!(
		"AWS4-HMAC-SHA256-PAYLOAD\n{amz_date}\n{scope}\n{prev}\n{EMPTY_SHA256}\n{}",
		sha256_hex(chunk_data)
	);
	hex::encode(hmac_sha256(signing_key, sts.as_bytes()))
}

/// Canonical URI: unreserved characters and `/` pass through, everything else
/// is `%XX`. (S3 signs the path with a single encoding.)
pub fn canonical_uri(path: &str) -> String {
	if path.is_empty() {
		return "/".into();
	}
	let mut out = String::with_capacity(path.len());
	for &b in path.as_bytes() {
		if is_unreserved(b) || b == b'/' {
			out.push(b as char);
		} else {
			push_pct(&mut out, b);
		}
	}
	out
}

/// Canonical query string: each key and value percent-encoded, pairs sorted by
/// encoded key. kopia forwards already-encoded parameters, so an existing `%`
/// triplet is passed through rather than re-encoded.
pub fn canonical_query(query: &str) -> String {
	if query.is_empty() {
		return String::new();
	}
	let mut pairs: Vec<(String, String)> = query
		.split('&')
		.map(|part| match part.split_once('=') {
			Some((k, v)) => (pct(k), pct(v)),
			None => (pct(part), String::new()),
		})
		.collect();
	pairs.sort();
	pairs
		.iter()
		.map(|(k, v)| format!("{k}={v}"))
		.collect::<Vec<_>>()
		.join("&")
}

/// Build the canonical-headers block and the signed-headers list from a set of
/// (name, value) pairs: names lowercased, values trimmed, sorted by name.
pub fn canonical_headers(headers: &[(String, String)]) -> (String, String) {
	let mut sorted: Vec<(String, String)> = headers
		.iter()
		.map(|(n, v)| (n.to_ascii_lowercase(), v.trim().to_string()))
		.collect();
	sorted.sort_by(|a, b| a.0.cmp(&b.0));
	let canonical = sorted.iter().map(|(n, v)| format!("{n}:{v}\n")).collect();
	let signed = sorted
		.iter()
		.map(|(n, _)| n.as_str())
		.collect::<Vec<_>>()
		.join(";");
	(canonical, signed)
}

fn pct(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for &b in s.as_bytes() {
		if is_unreserved(b) {
			out.push(b as char);
		} else if b == b'%' {
			// Assume an already-encoded triplet from minio-go; don't double-encode.
			out.push('%');
		} else {
			push_pct(&mut out, b);
		}
	}
	out
}

fn push_pct(out: &mut String, b: u8) {
	const HEX: &[u8; 16] = b"0123456789ABCDEF";
	out.push('%');
	out.push(HEX[(b >> 4) as usize] as char);
	out.push(HEX[(b & 0xf) as usize] as char);
}

fn is_unreserved(b: u8) -> bool {
	b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

#[cfg(test)]
mod tests {
	use super::*;

	/// AWS SigV4 test suite: `get-vanilla`.
	#[test]
	fn aws_get_vanilla_vector() {
		let secret = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
		let (region, service, date_stamp, amz_date) =
			("us-east-1", "service", "20150830", "20150830T123600Z");
		let key = signing_key(secret, date_stamp, region, service);
		let headers = [
			("Host".to_string(), "example.amazonaws.com".to_string()),
			("X-Amz-Date".to_string(), amz_date.to_string()),
		];
		let (ch, sh) = canonical_headers(&headers);
		assert_eq!(sh, "host;x-amz-date");
		let cr = canonical_request(
			"GET",
			&canonical_uri("/"),
			&canonical_query(""),
			&ch,
			&sh,
			EMPTY_SHA256,
		);
		let sts = string_to_sign(amz_date, &scope(date_stamp, region, service), &cr);
		assert_eq!(
			signature(&key, &sts),
			"5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
		);
	}

	// AWS docs: "Signature Calculations … Transferring Payload in Multiple
	// Chunks" (the canonical streaming-PUT example).
	const S3_SECRET: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
	const S3_DATE_STAMP: &str = "20130524";
	const S3_AMZ_DATE: &str = "20130524T000000Z";
	const S3_SEED: &str = "4f232c4386841ef735655705268965c44a0e4690baa4adea153f7db9fa80a0a9";

	#[test]
	fn aws_s3_streaming_seed_signature_vector() {
		let key = signing_key(S3_SECRET, S3_DATE_STAMP, "us-east-1", "s3");
		let headers = [
			("content-encoding", "aws-chunked"),
			("content-length", "66824"),
			("host", "s3.amazonaws.com"),
			("x-amz-content-sha256", STREAMING_PAYLOAD),
			("x-amz-date", S3_AMZ_DATE),
			("x-amz-decoded-content-length", "66560"),
			("x-amz-storage-class", "REDUCED_REDUNDANCY"),
		]
		.map(|(n, v)| (n.to_string(), v.to_string()));
		let (ch, sh) = canonical_headers(&headers);
		let cr = canonical_request(
			"PUT",
			&canonical_uri("/examplebucket/chunkObject.txt"),
			"",
			&ch,
			&sh,
			STREAMING_PAYLOAD,
		);
		let sts = string_to_sign(S3_AMZ_DATE, &scope(S3_DATE_STAMP, "us-east-1", "s3"), &cr);
		assert_eq!(signature(&key, &sts), S3_SEED);
	}

	#[test]
	fn aws_s3_streaming_chunk_chain_vector() {
		let key = signing_key(S3_SECRET, S3_DATE_STAMP, "us-east-1", "s3");
		let sc = scope(S3_DATE_STAMP, "us-east-1", "s3");
		let chunk1 = chunk_signature(&key, S3_AMZ_DATE, &sc, S3_SEED, &[b'a'; 65536]);
		assert_eq!(
			chunk1,
			"ad80c730a21e5b8d04586a2213dd63b9a0e99e0e2307b0ade35a65485a288648"
		);
		let chunk2 = chunk_signature(&key, S3_AMZ_DATE, &sc, &chunk1, &[b'a'; 1024]);
		assert_eq!(
			chunk2,
			"0055627c9e194cb4542bae2aa5492e3c1575bbb81b612b7d234b86a503ef5497"
		);
		let last = chunk_signature(&key, S3_AMZ_DATE, &sc, &chunk2, b"");
		assert_eq!(
			last,
			"b6c6ea8a5354eaf15b3cb7646744f4275b71ea724fed81ceb9323e279d449df9"
		);
	}

	#[test]
	fn canonical_query_sorts_and_encodes() {
		assert_eq!(
			canonical_query("list-type=2&prefix=a/b&marker="),
			"list-type=2&marker=&prefix=a%2Fb"
		);
	}

	#[test]
	fn canonical_uri_preserves_slashes_encodes_others() {
		assert_eq!(canonical_uri("/bucket/a b"), "/bucket/a%20b");
		assert_eq!(canonical_uri(""), "/");
	}
}
