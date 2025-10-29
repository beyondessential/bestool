use std::sync::Arc;

use miette::{IntoDiagnostic, Result};
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::ClientConfig;
use tokio_postgres_rustls::MakeRustlsConnect;
use tracing::debug;

pub type MakeRustlsConnectWrapper = MakeRustlsConnect;

/// Create a TLS connector using rustls with system certificate store
pub fn make_tls_connector() -> Result<MakeRustlsConnect> {
	debug!("creating TLS connector with system certificates");

	let mut root_store = rustls::RootCertStore::empty();

	let native_certs = rustls_native_certs::load_native_certs();
	debug!(
		count = native_certs.certs.len(),
		"loaded native certificates"
	);

	for cert in native_certs.certs {
		root_store.add(cert).into_diagnostic()?;
	}

	let config = ClientConfig::builder()
		.with_root_certificates(root_store)
		.with_no_client_auth();

	Ok(MakeRustlsConnect::new(config))
}

/// Create a TLS connector that accepts invalid certificates (for development)
#[allow(dead_code)]
pub fn make_insecure_tls_connector() -> Result<MakeRustlsConnect> {
	debug!("creating insecure TLS connector (accepts invalid certificates)");

	let config = ClientConfig::builder()
		.dangerous()
		.with_custom_certificate_verifier(Arc::new(NoVerifier))
		.with_no_client_auth();

	Ok(MakeRustlsConnect::new(config))
}

#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
	fn verify_server_cert(
		&self,
		_end_entity: &CertificateDer<'_>,
		_intermediates: &[CertificateDer<'_>],
		_server_name: &ServerName<'_>,
		_ocsp_response: &[u8],
		_now: rustls::pki_types::UnixTime,
	) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
		Ok(rustls::client::danger::ServerCertVerified::assertion())
	}

	fn verify_tls12_signature(
		&self,
		_message: &[u8],
		_cert: &CertificateDer<'_>,
		_dss: &rustls::DigitallySignedStruct,
	) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
		Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
	}

	fn verify_tls13_signature(
		&self,
		_message: &[u8],
		_cert: &CertificateDer<'_>,
		_dss: &rustls::DigitallySignedStruct,
	) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
		Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
	}

	fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
		vec![
			rustls::SignatureScheme::RSA_PKCS1_SHA256,
			rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
			rustls::SignatureScheme::ED25519,
		]
	}
}
