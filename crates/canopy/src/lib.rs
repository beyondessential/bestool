use std::fmt;

mod client;
pub mod registration;

pub use client::{
	CERT_RENEW_AFTER, CanopyClient, ClientBuilderFactory, DEFAULT_CANOPY_URL, NewEvent, Severity,
	TAILSCALE_URL, client_builder, device_identity, user_agent,
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
