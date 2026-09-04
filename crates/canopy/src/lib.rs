//! bestool's canopy client: the published [`bes_canopy_api`] wire layer plus
//! bestool's own HTTP transport and registration/backup helpers.
//!
//! The typed [`CanopyClient`], the [`CanopyTransport`] trait, the wire types in
//! [`schema`], and the error types all come from [`bes_canopy_api`] and are
//! re-exported here. This crate supplies the parts specific to how bestool
//! reaches canopy:
//!
//! - [`ReqwestTransport`], the default [`CanopyTransport`], which picks canopy's
//!   tailscale or mTLS auth path and routes calls accordingly;
//! - [`connect`] and [`connect_to`], which probe for an auth path and build a
//!   [`CanopyClient`] over one;
//! - [`registration`], and the backup helpers [`TargetOutcome`] and
//!   [`ContainerCreds`].
//!
//! The transport-shaped operations — [`is_tailscale`](ReqwestTransport::is_tailscale),
//! [`refresh`](ReqwestTransport::refresh), [`renew`](ReqwestTransport::renew) —
//! live on [`ReqwestTransport`]; reach them through
//! [`CanopyClient::transport`](bes_canopy_api::CanopyClient::transport).
//!
//! # Wire types
//!
//! The types in [`schema`] are generated from canopy's OpenAPI document, which
//! canopy builds and publishes as `bes-canopy-api`. Timestamp fields are
//! [`jiff::Timestamp`], credential secrets are wrapped in [`Redacted`] so they
//! stay out of `Debug` output, and each generated struct carries a builder and
//! is `#[non_exhaustive]`. [`CanopyClient`] has one method per endpoint taking
//! and returning these types; any non-2xx surfaces as [`CanopyHttpError`].

mod backup;
mod connect;
pub mod registration;
mod reqwest_transport;
#[cfg(test)]
mod test_support;

pub use bes_canopy_api::{
	CanopyHttpError, CanopyRequest, CanopyResponse, CanopyTransport, Error, Redacted, async_trait,
	bytes, http, schema,
};

pub use backup::{ContainerCreds, TargetOutcome};
pub use connect::{connect, connect_to};
pub use reqwest;
pub use reqwest_transport::{
	CERT_RENEW_AFTER, ClientBuilderFactory, DEFAULT_CANOPY_URL, ReqwestTransport, TAILSCALE_URL,
	device_identity, tailscale_client,
};

/// The typed canopy client, defaulting to bestool's [`ReqwestTransport`].
///
/// [`bes_canopy_api::CanopyClient`] takes its transport as a required type
/// parameter; this alias restores the default so the common case — a client
/// built by [`connect`] or [`connect_to`] — never has to name it.
pub type CanopyClient<T = ReqwestTransport> = bes_canopy_api::CanopyClient<T>;
