//! Building a [`CanopyClient`] over the default [`ReqwestTransport`].
//!
//! These are the constructors the common case reaches for: they probe canopy's
//! auth paths (tailscale first, then mTLS from a device key) and, when one is
//! available, wrap the resulting [`ReqwestTransport`] in a [`CanopyClient`]. A
//! caller reaching canopy some other way builds its own transport and passes it
//! to [`CanopyClient::new`](bes_canopy_api::CanopyClient::new) directly.

use miette::Result;
use reqwest::Url;

use crate::{CanopyClient, ReqwestTransport};

/// Build a canopy client against the default public
/// ([`DEFAULT_CANOPY_URL`](crate::DEFAULT_CANOPY_URL)) and tailscale
/// ([`TAILSCALE_URL`](crate::TAILSCALE_URL)) endpoints. Use [`connect_to`] to
/// override them.
///
/// Probes the tailscale endpoint first; if reachable, uses it. Otherwise, if a
/// device key PEM is provided, builds an mTLS client. Returns `Ok(None)` if
/// neither path is available.
///
/// `make_builder` supplies the base [`reqwest::ClientBuilder`] — see
/// [`ClientBuilderFactory`](crate::ClientBuilderFactory).
pub async fn connect(
	device_key_pem: Option<&str>,
	make_builder: impl Fn() -> reqwest::ClientBuilder + Send + Sync + 'static,
) -> Result<Option<CanopyClient>> {
	connect_to(
		crate::DEFAULT_CANOPY_URL
			.parse()
			.expect("default canopy URL is valid"),
		crate::TAILSCALE_URL
			.parse()
			.expect("default tailscale URL is valid"),
		device_key_pem,
		make_builder,
	)
	.await
}

/// Build a canopy client against explicit endpoints.
///
/// `base_url` is canopy's public API URL (the registration's `api_url`), used on
/// the mTLS path; `tailscale_url` is the tailnet endpoint used on the tailscale
/// path. Both are fixed for the client's lifetime. See [`connect`] for the other
/// arguments and the default-endpoint form.
pub async fn connect_to(
	base_url: Url,
	tailscale_url: Url,
	device_key_pem: Option<&str>,
	make_builder: impl Fn() -> reqwest::ClientBuilder + Send + Sync + 'static,
) -> Result<Option<CanopyClient>> {
	Ok(
		ReqwestTransport::new(base_url, tailscale_url, device_key_pem, make_builder)
			.await?
			.map(CanopyClient::new),
	)
}

#[cfg(test)]
mod tests {
	use crate::{
		DEFAULT_CANOPY_URL,
		test_support::{closed_url, serve_once},
	};

	use super::*;

	/// The default-transport constructor and the auth-path methods that delegate
	/// to it, reached through `transport()`; what the transport itself decides is
	/// covered in `reqwest_transport`.
	#[tokio::test]
	async fn connect_to_builds_on_the_default_transport() {
		let (tailnet, _server) = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]");
		let client = connect_to(
			DEFAULT_CANOPY_URL.parse().unwrap(),
			tailnet.parse().unwrap(),
			None,
			reqwest::Client::builder,
		)
		.await
		.expect("keyless build should not error")
		.expect("a reachable tailnet is an auth path in its own right");
		assert!(client.transport().is_tailscale().await);
		client
			.transport()
			.renew()
			.await
			.expect("renew should be a no-op");
	}

	#[tokio::test]
	async fn connect_to_yields_no_client_without_an_auth_path() {
		let client = connect_to(
			DEFAULT_CANOPY_URL.parse().unwrap(),
			closed_url().parse().unwrap(),
			None,
			reqwest::Client::builder,
		)
		.await
		.expect("keyless build should not error");
		assert!(client.is_none());
	}
}
