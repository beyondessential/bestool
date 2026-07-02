use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod backup;
mod client;
pub mod registration;
mod restore;

/// Wire types generated at build time from canopy's OpenAPI document.
///
/// These track canopy's API as it evolves: the build script fetches the live
/// spec (falling back to a committed snapshot) and regenerates on each build,
/// so nothing here is hand-maintained or committed. Use them with the generic
/// [`CanopyClient::request`]/[`CanopyClient::request_json`] methods; the
/// bespoke endpoint methods keep their own hand-written types.
pub mod schema {
	include!(concat!(env!("OUT_DIR"), "/canopy_schema.rs"));
}

pub use backup::{
	BackupCredentials, BackupCredentialsRequest, BackupReport, BackupTarget, CapabilitiesRequest,
	ContainerCreds, Outcome, Purpose, TargetOutcome,
};
pub use client::{
	CERT_RENEW_AFTER, CanopyClient, ClientBuilderFactory, DEFAULT_CANOPY_URL, NewEvent, Severity,
	TAILSCALE_URL, client_builder, device_identity, tailscale_client, user_agent,
};
pub use restore::{
	RestoreCapabilitiesRequest, RestoreCredentials, RestoreCredentialsRequest, RestoreVerification,
	WorklistEntry,
};

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
