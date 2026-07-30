//! The HTTP layer underneath [`CanopyClient`](crate::CanopyClient).
//!
//! Everything above this layer — the generated wire types, the per-endpoint
//! methods, gzipping, status handling, JSON parsing — is transport-agnostic. A
//! caller that can't (or doesn't want to) reach canopy over
//! [`ReqwestTransport`](crate::ReqwestTransport), the default, implements
//! [`CanopyTransport`] and keeps the whole typed interface on top of it.

use std::sync::Arc;

use bytes::Bytes;
use miette::Result;

/// A request built by [`CanopyClient`](crate::CanopyClient), ready for a
/// [`CanopyTransport`] to send.
///
/// The URI is the endpoint **path** in origin form (path plus query, no scheme
/// or authority), e.g. `/backup-target` — resolving it against a base URL is
/// the transport's job. The body is already serialised and gzipped when there is
/// one (with `content-type` and `content-encoding` set to match) and empty when
/// there isn't.
pub type CanopyRequest = http::Request<Bytes>;

/// A response handed back to [`CanopyClient`](crate::CanopyClient) by a
/// [`CanopyTransport`], with its body buffered.
///
/// The status is interpreted by the client: a non-2xx becomes a
/// [`CanopyHttpError`](crate::CanopyHttpError) carrying the body, and a success
/// has its body parsed into the endpoint's response type.
pub type CanopyResponse = http::Response<Bytes>;

/// The HTTP transport a [`CanopyClient`](crate::CanopyClient) sends through.
///
/// Implement this to route canopy calls somewhere of your own choosing — a
/// proxy that isn't a plain HTTP proxy, an in-process handler, a recorded
/// fixture in tests — and pass it to
/// [`CanopyClient::with_transport`](crate::CanopyClient::with_transport). The
/// generated per-endpoint methods, the wire types, and the error handling all
/// work unchanged on top; callers who don't need this get
/// [`ReqwestTransport`](crate::ReqwestTransport) and never see this trait.
///
/// # Contract
///
/// - Requests arrive with a path-only URI (see [`CanopyRequest`]); the transport
///   decides what host, scheme, and auth to use, and may rewrite the path (the
///   default transport prefixes `/public` when it goes over the tailnet).
/// - Return canopy's response as-is, non-2xx included: statuses are the client's
///   to interpret, since endpoints give meaning to specific codes (e.g. a
///   backup-target `412` means the device is dormant, see
///   [`TargetOutcome::from_result`](crate::TargetOutcome::from_result)).
/// - `Err` is for a failure to obtain any response at all (connect, timeout,
///   protocol error).
///
/// # Example
///
/// ```no_run
/// use bestool_canopy::{
///     CanopyClient, CanopyRequest, CanopyResponse, CanopyTransport, async_trait,
/// };
/// use miette::Result;
///
/// struct MyProxy;
///
/// #[async_trait]
/// impl CanopyTransport for MyProxy {
///     async fn call(&self, request: CanopyRequest) -> Result<CanopyResponse> {
///         // Hand `request` to whatever reaches canopy from here, and return
///         // what comes back.
///         todo!()
///     }
/// }
///
/// # async fn example() -> Result<()> {
/// let client = CanopyClient::with_transport(MyProxy);
/// let servers = client.servers().await?;
/// # Ok(())
/// # }
/// ```
#[async_trait::async_trait]
pub trait CanopyTransport: Send + Sync {
	/// Send `request` and return canopy's response.
	async fn call(&self, request: CanopyRequest) -> Result<CanopyResponse>;
}

#[async_trait::async_trait]
impl<T: CanopyTransport + ?Sized> CanopyTransport for Arc<T> {
	async fn call(&self, request: CanopyRequest) -> Result<CanopyResponse> {
		(**self).call(request).await
	}
}

#[async_trait::async_trait]
impl<T: CanopyTransport + ?Sized> CanopyTransport for Box<T> {
	async fn call(&self, request: CanopyRequest) -> Result<CanopyResponse> {
		(**self).call(request).await
	}
}
