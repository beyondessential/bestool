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

/// Loopback base URLs the alertd daemon may be listening on.
///
/// The daemon binds every loopback address it can, but on a host where only one
/// family is available it ends up on just that one, so a client must try both.
/// Kept in step with alertd's `default_server_addrs`.
pub(crate) const DAEMON_BASES: [&str; 2] = ["http://[::1]:8271", "http://127.0.0.1:8271"];

/// GET `path` from the alertd daemon, trying each base in [`DAEMON_BASES`] in
/// turn until one connects.
///
/// `configure` is applied to each request builder (timeouts, etc.). Returns the
/// first response whose connection succeeded; the `Err` carries the last
/// connection error, meaning no daemon answered on any base.
pub(crate) async fn daemon_get(
	client: &reqwest::Client,
	path: &str,
	configure: impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> reqwest::Result<reqwest::Response> {
	let mut last_err = None;
	for base in DAEMON_BASES {
		let request = configure(client.get(format!("{base}{path}")));
		match request.send().await {
			Ok(response) => return Ok(response),
			Err(err) => last_err = Some(err),
		}
	}
	Err(last_err.expect("DAEMON_BASES is never empty"))
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
