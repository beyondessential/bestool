use rustls::ClientConfig;
use tokio_postgres_rustls::MakeRustlsConnect;
use tracing::debug;

/// Create a TLS connector using rustls with system certificate store
pub fn make_tls_connector() -> Result<MakeRustlsConnect, rustls::Error> {
	debug!("creating TLS connector with system certificates");

	let mut root_store = rustls::RootCertStore::empty();

	let native_certs = rustls_native_certs::load_native_certs();
	debug!(
		count = native_certs.certs.len(),
		"loaded native certificates"
	);

	for cert in native_certs.certs {
		root_store.add(cert)?;
	}

	let config = ClientConfig::builder()
		.with_root_certificates(root_store)
		.with_no_client_auth();

	Ok(MakeRustlsConnect::new(config))
}
