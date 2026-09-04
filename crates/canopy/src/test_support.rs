//! Helpers shared by the unit tests in this crate.

use crate::ClientBuilderFactory;
use std::sync::Arc;

pub(crate) const TEST_DEVICE_KEY: &str = "\
-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgVvhzsYiidp38GYn1
KxD5Wipc/h8lglVsy1UFZq/SZbGhRANCAAT2EsEq7xjeWVnim9XwdYXga/LBbppm
fXLgamTYOa/w9n/Ta64fiYWmN54kEd0DgnflJDLtID321Zz6xswvK/VN
-----END PRIVATE KEY-----";

pub(crate) fn test_factory() -> ClientBuilderFactory {
	Arc::new(reqwest::Client::builder)
}

/// A loopback URL nothing is listening on, so a probe refuses immediately.
pub(crate) fn closed_url() -> String {
	let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
	let addr = listener.local_addr().unwrap();
	drop(listener);
	format!("http://{addr}")
}

pub(crate) struct Captured {
	pub(crate) request_line: String,
	pub(crate) headers: String,
}

/// Bind a loopback socket and answer exactly one HTTP request with
/// `response`, capturing the received request line, headers, and body.
pub(crate) fn serve_once(response: &'static str) -> (String, std::thread::JoinHandle<Captured>) {
	use std::io::{Read, Write};
	use std::net::TcpListener;

	let listener = TcpListener::bind("127.0.0.1:0").unwrap();
	let base = format!("http://{}", listener.local_addr().unwrap());
	let handle = std::thread::spawn(move || {
		let (mut stream, _) = listener.accept().unwrap();
		let mut buf = Vec::new();
		let mut chunk = [0u8; 1024];
		let header_end = loop {
			if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
				break pos + 4;
			}
			let n = stream.read(&mut chunk).unwrap();
			if n == 0 {
				panic!("connection closed before headers were complete");
			}
			buf.extend_from_slice(&chunk[..n]);
		};

		let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
		let content_length = head
			.lines()
			.find_map(|line| {
				let (name, value) = line.split_once(':')?;
				name.trim()
					.eq_ignore_ascii_case("content-length")
					.then(|| value.trim().parse::<usize>().ok())
					.flatten()
			})
			.unwrap_or(0);

		// Drain the request body so the client's send completes before we reply.
		let mut drained = buf[header_end..].len();
		while drained < content_length {
			let n = stream.read(&mut chunk).unwrap();
			if n == 0 {
				break;
			}
			drained += n;
		}

		stream.write_all(response.as_bytes()).unwrap();
		stream.flush().unwrap();

		let mut lines = head.lines();
		let request_line = lines.next().unwrap_or_default().to_owned();
		let headers = lines.collect::<Vec<_>>().join("\n");
		Captured {
			request_line,
			headers,
		}
	});
	(base, handle)
}
