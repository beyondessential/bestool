use std::sync::OnceLock;

/// Browser-style user-agent sent on every outbound HTTP request bestool makes.
///
/// `bestool/<version> (<os>; <arch>)`, e.g.
/// `bestool/1.18.1 (Linux 7.0.9 Arch Linux; x86_64)`. The OS comment is
/// detected at runtime and cached.
pub(crate) fn user_agent() -> &'static str {
	static UA: OnceLock<String> = OnceLock::new();
	UA.get_or_init(|| {
		let os = sysinfo::System::long_os_version()
			.or_else(sysinfo::System::name)
			.unwrap_or_else(|| std::env::consts::OS.to_owned());
		format!(
			"bestool/{} ({os}; {})",
			env!("CARGO_PKG_VERSION"),
			sysinfo::System::cpu_arch(),
		)
	})
}

/// Base builder for all of bestool's `reqwest` clients.
///
/// Sets the [`user_agent`] and opts into honouring `SSLKEYLOGFILE` (a no-op
/// unless that env var is set at runtime). Call sites add their own timeouts,
/// DNS overrides, etc. on top.
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
	reqwest::Client::builder()
		.user_agent(user_agent())
		.tls_sslkeylogfile(true)
}

/// A built [`reqwest::Client`] from [`client_builder`] with default settings.
pub(crate) fn client() -> reqwest::Client {
	client_builder()
		.build()
		.expect("failed to build bestool HTTP client")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn user_agent_has_product_and_os_comment() {
		let ua = user_agent();
		assert!(ua.starts_with("bestool/"), "unexpected user-agent: {ua}");
		assert!(ua.contains('('), "expected OS comment in: {ua}");
		assert!(ua.ends_with(')'), "expected OS comment in: {ua}");
		assert!(
			ua.contains(sysinfo::System::cpu_arch().as_str()),
			"expected arch in: {ua}"
		);
	}
}
