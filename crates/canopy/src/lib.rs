use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod backup;
mod client;
pub mod registration;
mod restore;

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
