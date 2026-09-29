//! The HTTP client for services on this machine: Tamanu's API behind Caddy, and
//! Caddy's admin API.
//!
//! It keeps no idle connections, so every request opens a new one. Requests to
//! these services come a sweep apart, and on some hosts a connection left idle
//! that long is dropped without being closed, plausibly by endpoint security
//! that intercepts loopback HTTP. A request sent on such a connection hangs
//! until its timeout, then the next one, on a fresh connection, succeeds, so a
//! pooled client makes a healthy service fail every other sweep. A loopback
//! connect costs next to nothing, so there is no warmth worth keeping.
//!
//! spec: CHK#services-on-this-machine

use std::sync::LazyLock;

/// The shared client for services on this machine.
pub fn client() -> &'static reqwest::Client {
	static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
		reqwest::Client::builder()
			.user_agent(crate::USER_AGENT)
			.pool_max_idle_per_host(0)
			.build()
			.expect("failed to build alertd local HTTP client")
	});
	&CLIENT
}

#[cfg(test)]
mod tests {
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};

	use tokio::{
		io::{AsyncReadExt, AsyncWriteExt},
		net::TcpListener,
	};

	/// A server that offers keep-alive and would happily serve every request on
	/// one connection still sees a new connection per request.
	#[tokio::test]
	async fn each_request_opens_a_new_connection() {
		let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		let accepted = Arc::new(AtomicUsize::new(0));
		let counted = accepted.clone();
		tokio::spawn(async move {
			while let Ok((mut stream, _)) = listener.accept().await {
				counted.fetch_add(1, Ordering::SeqCst);
				tokio::spawn(async move {
					let mut scratch = [0u8; 1024];
					while let Ok(n) = stream.read(&mut scratch).await {
						if n == 0 {
							break;
						}
						let _ = stream
							.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
							.await;
					}
				});
			}
		});

		let url = format!("http://{addr}/");
		for _ in 0..3 {
			let resp = super::client().get(&url).send().await.unwrap();
			assert_eq!(resp.text().await.unwrap(), "ok");
		}
		assert_eq!(accepted.load(Ordering::SeqCst), 3);
	}
}
