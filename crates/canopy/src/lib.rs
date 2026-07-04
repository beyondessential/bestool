use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod backup;
mod client;
pub mod registration;

/// Wire types generated at build time from canopy's OpenAPI document.
///
/// These are the canonical request and response types for canopy's API, and the
/// ones to reach for first. The build script fetches the live spec and
/// regenerates them, so they track canopy as it evolves and nothing here is
/// hand-maintained or committed. Each type carries the schema's own description
/// as rustdoc. (A failed fetch fails the build rather than silently using the
/// committed snapshot, which is reserved for docs.rs and explicit offline
/// builds — see the build script.)
///
/// Naming follows canopy's schema: request bodies are `…Args` (e.g.
/// [`BackupCredentialsArgs`], [`ReportArgs`], [`BackupCapabilitiesArgs`]), and
/// credentials come back as [`CredentialProcessOutput`].
///
/// The generated source is rewritten in two ways the raw JSON Schema can't
/// express (see the build script): timestamp fields are [`jiff::Timestamp`]
/// rather than strings, and credential secrets (`secret_access_key`,
/// `session_token`, `repo_password`) are wrapped in [`Redacted`] so they never
/// surface in `Debug` output or logs — read them through the inner value.
///
/// [`CanopyClient`] has one generated method per endpoint (also emitted from the
/// spec into this module — e.g. `backup_credentials`, `restore_worklist`,
/// `tags`), taking and returning these types; that's how you call canopy. The
/// method name is the path (`/backup-credentials` → `backup_credentials`), verb-
/// prefixed only where a path is served by several verbs. `backup_target`'s
/// dormant-device case is read from its result via [`TargetOutcome::from_result`];
/// any non-2xx surfaces as [`CanopyHttpError`]. The generic
/// `get`/`request`/`request_json` escape hatch is behind the off-by-default
/// `raw-requests` feature — reach for it only for something the generated methods
/// don't cover.
///
/// [`BackupCredentialsArgs`]: schema::BackupCredentialsArgs
/// [`ReportArgs`]: schema::ReportArgs
/// [`BackupCapabilitiesArgs`]: schema::BackupCapabilitiesArgs
/// [`CredentialProcessOutput`]: schema::CredentialProcessOutput
pub mod schema {
	include!(concat!(env!("OUT_DIR"), "/canopy_schema.rs"));
}

pub use backup::{ContainerCreds, TargetOutcome};
pub use client::{
	CERT_RENEW_AFTER, CanopyClient, CanopyHttpError, ClientBuilderFactory, DEFAULT_CANOPY_URL,
	TAILSCALE_URL, device_identity, tailscale_client,
};
pub use reqwest;

/// Wraps a sensitive value so its `Debug` output doesn't leak the contents.
#[derive(Clone)]
pub struct Redacted<T>(pub T);

impl<T> fmt::Debug for Redacted<T> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("<redacted>")
	}
}

impl<T> std::ops::Deref for Redacted<T> {
	type Target = T;
	fn deref(&self) -> &T {
		&self.0
	}
}

impl<T: Serialize> Serialize for Redacted<T> {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		self.0.serialize(serializer)
	}
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Redacted<T> {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		T::deserialize(deserializer).map(Redacted)
	}
}
