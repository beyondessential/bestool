use std::{path::PathBuf, sync::Arc, time::Duration};

use algae_cli::{
	passphrases::{Passphrase, PassphraseArgs},
	streams::decrypt_stream,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use bestool_canopy::{
	TAILSCALE_URL, device_identity,
	registration::{self, Registration},
	tailscale_client,
};
use bestool_tamanu::server_info::generate_device_key_pem;
use clap::Parser;
use miette::{IntoDiagnostic as _, Result, WrapErr as _, bail};
use p256::{
	SecretKey,
	ecdsa::{Signature, SigningKey, signature::Signer as _},
	elliptic_curve::pkcs8::{DecodePrivateKey as _, EncodePublicKey as _},
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tracing::debug;
use uuid::Uuid;

use crate::actions::Context;

/// Enrol this machine as a Canopy server.
///
/// An operator first creates the server record in Canopy, which hands back an
/// encrypted enrollment ticket plus a separate passphrase (shared out of band).
/// This command decrypts the ticket, then claims the pre-created server over
/// mTLS by proving the machine holds the private key behind the certificate it
/// presents. On success the device key, server id, device id, and api url are
/// stored in the machine-bound encrypted registration.
#[derive(Debug, Clone, Parser)]
pub struct RegisterArgs {
	/// Encrypted enrollment ticket from Canopy.
	///
	/// Copy-paste the whole `bestool canopy register <ticket>` line Canopy
	/// shows you. The ticket is encrypted, so it's safe to pass on the command
	/// line. If omitted, the ticket is read from stdin.
	pub ticket: Option<String>,

	/// Directory holding the encrypted canopy registration.
	///
	/// Defaults to the platform's machine-global config directory
	/// (`/etc/bestool`, or `%ProgramData%\bestool` on Windows).
	#[arg(long, value_name = "DIR")]
	pub config: Option<PathBuf>,

	#[command(flatten)]
	#[allow(missing_docs, reason = "don't interfere with clap")]
	pub passphrase: PassphraseArgs,
}

/// The decrypted enrollment ticket payload.
///
/// No `Debug` derive on purpose: `token` is a bearer secret and must never be
/// logged.
#[derive(Deserialize)]
struct EnrollTicket {
	v: String,
	api_url: String,
	server_id: String,
	token: String,
}

#[derive(Serialize)]
pub(crate) struct BeginRequest<'a> {
	pub(crate) server_id: &'a str,
	pub(crate) token: &'a str,
	/// DER SubjectPublicKeyInfo (base64), sent only over the tailscale path
	/// where there's no client cert for canopy to read it from.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub(crate) spki: Option<&'a str>,
}

#[derive(Deserialize)]
pub(crate) struct BeginResponse {
	pub(crate) nonce: String,
	#[serde(default)]
	pub(crate) channel_binding_required: bool,
}

#[derive(Serialize)]
pub(crate) struct CompleteRequest<'a> {
	pub(crate) server_id: &'a str,
	pub(crate) nonce: &'a str,
	pub(crate) signature: &'a str,
	/// DER SubjectPublicKeyInfo (base64), sent only over the tailscale path
	/// where there's no client cert for canopy to read it from.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub(crate) spki: Option<&'a str>,
}

/// Which network path the enrollment handshake takes.
///
/// Mirrors the rest of bestool's canopy traffic: prefer the tailnet when it's
/// reachable, fall back to the public mTLS interface otherwise.
enum Transport {
	/// Reachable over the canopy tailnet: plain HTTPS to `/public/...`, no
	/// client cert (tailnet identity authenticates), device SPKI carried in the
	/// `complete` body.
	Tailscale(reqwest::Client),
	/// Public mTLS to the ticket's `api_url`: the client cert is presented and
	/// canopy reads the SPKI from it.
	Mtls(reqwest::Client),
}

impl Transport {
	fn client(&self) -> &reqwest::Client {
		match self {
			Transport::Tailscale(c) | Transport::Mtls(c) => c,
		}
	}

	fn carries_spki_in_body(&self) -> bool {
		matches!(self, Transport::Tailscale(_))
	}

	fn url(&self, api_url: &Url, step: &str) -> Result<Url> {
		match self {
			Transport::Tailscale(_) => format!("{TAILSCALE_URL}/public/servers/register/{step}")
				.parse()
				.into_diagnostic()
				.wrap_err_with(|| format!("building tailscale register/{step} URL")),
			Transport::Mtls(_) => api_url
				.join(&format!("/servers/register/{step}"))
				.into_diagnostic()
				.wrap_err_with(|| format!("building register/{step} URL")),
		}
	}
}

#[derive(Deserialize)]
pub(crate) struct CompleteResponse {
	pub(crate) server_id: String,
	pub(crate) device_id: String,
}

/// RFC-7807-style problem body. Canopy's register errors are intentionally
/// opaque, so we just surface whatever `title`/`detail` it gives us.
#[derive(Deserialize)]
struct Problem {
	title: Option<String>,
	detail: Option<String>,
}

pub async fn run(args: RegisterArgs, _ctx: Context) -> Result<()> {
	let RegisterArgs {
		ticket,
		config,
		passphrase,
	} = args;

	let dir = config.clone().unwrap_or_else(registration::default_dir);
	// Elevate now if we can't write the registration — before prompting for a
	// passphrase, and before the enrollment token is consumed over the network.
	super::ensure_writable_or_reexec(&dir)?;

	let ticket_b64 = match ticket {
		Some(t) => t,
		None => super::read_stdin("ticket")?,
	};
	let encrypted = super::decode_base64(ticket_b64.trim())?;

	let pass = passphrase.require().await?;
	let ticket = decrypt_ticket(&encrypted, pass).await?;

	if ticket.v != "enroll-1" {
		bail!(
			"unsupported enrollment ticket version {:?} (expected \"enroll-1\")",
			ticket.v
		);
	}

	let api_url: Url = ticket
		.api_url
		.parse()
		.into_diagnostic()
		.wrap_err_with(|| format!("ticket api_url is not a valid URL: {}", ticket.api_url))?;
	if api_url.scheme() != "https" {
		bail!("ticket api_url must be https, got {:?}", api_url.scheme());
	}
	let server_id = Uuid::parse_str(&ticket.server_id)
		.into_diagnostic()
		.wrap_err("ticket server_id is not a valid UUID")?;

	debug!(%api_url, %server_id, "decrypted enrollment ticket");

	let existing = super::load_registration(config.as_deref())
		.await
		.wrap_err("reading existing canopy registration")?;

	// Idempotency: if we've already enrolled this server with our identity, the
	// token has been consumed and re-running would only fail opaquely. Treat a
	// matching local record as success.
	if let Some(reg) = &existing
		&& reg.server_id.as_deref() == Some(ticket.server_id.as_str())
		&& let Some(device_id) = &reg.device_id
	{
		println!("Already enrolled with Canopy.");
		println!("  server id: {}", ticket.server_id);
		println!("  device id: {device_id}");
		return Ok(());
	}

	// Reuse the device key from an existing registration if present, else mint
	// one. Needed in both transports: the signature and the SPKI derive from it.
	let device_key_pem = match existing.as_ref().and_then(|r| r.device_key.clone()) {
		Some(pem) => pem,
		None => generate_device_key_pem()?,
	};
	let spki_der = spki_der(&device_key_pem)?;
	let signing_key = SigningKey::from_pkcs8_pem(&device_key_pem)
		.into_diagnostic()
		.wrap_err("loading device key for signing")?;

	// Prefer the tailnet, like the rest of bestool's canopy traffic; fall back
	// to public mTLS against the ticket's api_url.
	let factory: bestool_canopy::ClientBuilderFactory = Arc::new(crate::http::client_builder);
	let transport = match tailscale_client(&factory).await {
		Some(client) => {
			debug!("enrolling over the canopy tailnet");
			Transport::Tailscale(client)
		}
		None => {
			debug!("tailnet unreachable; enrolling over public mTLS");
			let identity = device_identity(&device_key_pem)?;
			let client = crate::http::client_builder()
				.identity(identity)
				.use_rustls_tls()
				.timeout(Duration::from_secs(30))
				.build()
				.into_diagnostic()
				.wrap_err("building mTLS HTTP client")?;
			Transport::Mtls(client)
		}
	};

	let spki_b64 = STANDARD.encode(&spki_der);

	// Step 1: begin — fetch the challenge nonce. The token isn't consumed here.
	let begin = begin(
		&transport,
		&api_url,
		&ticket.server_id,
		&ticket.token,
		&spki_b64,
	)
	.await?;
	if begin.channel_binding_required {
		bail!(
			"this Canopy server requires TLS channel binding, which this version of bestool does not support yet"
		);
	}
	let nonce_bytes = STANDARD
		.decode(begin.nonce.trim())
		.into_diagnostic()
		.wrap_err("decoding challenge nonce")?;

	// Step 2: prove possession of the device key by signing the transcript.
	let transcript = build_transcript(&nonce_bytes, &server_id, &spki_der);
	let signature: Signature = signing_key.sign(&transcript);
	let signature_b64 = STANDARD.encode(signature.to_der().as_bytes());

	let complete = complete(
		&transport,
		&api_url,
		&ticket.server_id,
		&begin.nonce,
		&signature_b64,
		&spki_b64,
	)
	.await?;

	// Persist the result so the agent knows it's bound and where to report.
	let registration = Registration {
		server_id: Some(complete.server_id.clone()),
		device_key: Some(device_key_pem),
		device_id: Some(complete.device_id.clone()),
		api_url: Some(api_url.to_string()),
		..Registration::default()
	};
	registration::store_in(&dir, &registration)
		.await
		.wrap_err("storing canopy registration")?;

	println!("Enrolled with Canopy.");
	println!("  server id: {}", complete.server_id);
	println!("  device id: {}", complete.device_id);
	Ok(())
}

async fn decrypt_ticket(encrypted: &[u8], pass: Passphrase) -> Result<EnrollTicket> {
	let reader = futures::io::Cursor::new(encrypted.to_vec());
	let mut plaintext: Vec<u8> = Vec::new();
	decrypt_stream(reader, &mut plaintext, Box::new(pass))
		.await
		.wrap_err("decrypting enrollment ticket (wrong passphrase?)")?;
	serde_json::from_slice(&plaintext)
		.into_diagnostic()
		.wrap_err("parsing decrypted enrollment ticket")
}

/// Derive the device certificate's DER SubjectPublicKeyInfo from its key PEM.
///
/// Canopy identifies the device by this SPKI; it's the same public key the
/// self-signed cert presents over mTLS.
fn spki_der(device_key_pem: &str) -> Result<Vec<u8>> {
	let secret = SecretKey::from_pkcs8_pem(device_key_pem)
		.into_diagnostic()
		.wrap_err("parsing device key")?;
	let der = secret
		.public_key()
		.to_public_key_der()
		.into_diagnostic()
		.wrap_err("encoding device public key (SPKI)")?;
	Ok(der.as_bytes().to_vec())
}

/// Build the proof-of-possession transcript Canopy expects at `complete`: the
/// raw challenge nonce, the server id's 16 UUID bytes, then the device
/// certificate's DER SubjectPublicKeyInfo.
///
/// The byte layout here and the signature encoding at the call site (DER
/// ECDSA, base64-standard) must match Canopy's verifier exactly.
fn build_transcript(nonce: &[u8], server_id: &Uuid, spki_der: &[u8]) -> Vec<u8> {
	let mut transcript = Vec::with_capacity(nonce.len() + 16 + spki_der.len());
	transcript.extend_from_slice(nonce);
	transcript.extend_from_slice(server_id.as_bytes());
	transcript.extend_from_slice(spki_der);
	transcript
}

async fn begin(
	transport: &Transport,
	api_url: &Url,
	server_id: &str,
	token: &str,
	spki: &str,
) -> Result<BeginResponse> {
	let url = transport.url(api_url, "begin")?;
	let resp = transport
		.client()
		.post(url)
		.json(&BeginRequest {
			server_id,
			token,
			spki: transport.carries_spki_in_body().then_some(spki),
		})
		.send()
		.await
		.into_diagnostic()
		.wrap_err("calling register/begin")?;
	parse_json_or_problem(resp, "register/begin").await
}

async fn complete(
	transport: &Transport,
	api_url: &Url,
	server_id: &str,
	nonce: &str,
	signature: &str,
	spki: &str,
) -> Result<CompleteResponse> {
	let url = transport.url(api_url, "complete")?;
	let resp = transport
		.client()
		.post(url)
		.json(&CompleteRequest {
			server_id,
			nonce,
			signature,
			spki: transport.carries_spki_in_body().then_some(spki),
		})
		.send()
		.await
		.into_diagnostic()
		.wrap_err("calling register/complete")?;
	parse_json_or_problem(resp, "register/complete").await
}

async fn parse_json_or_problem<T: serde::de::DeserializeOwned>(
	resp: reqwest::Response,
	what: &str,
) -> Result<T> {
	let status = resp.status();
	let body = resp
		.bytes()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("reading {what} response"))?;

	if status.is_success() {
		return serde_json::from_slice(&body)
			.into_diagnostic()
			.wrap_err_with(|| format!("parsing {what} response"));
	}

	if let Ok(problem) = serde_json::from_slice::<Problem>(&body) {
		let title = problem.title.unwrap_or_else(|| "enrollment failed".into());
		match problem.detail {
			Some(detail) => bail!("canopy {what} failed ({status}): {title}: {detail}"),
			None => bail!("canopy {what} failed ({status}): {title}"),
		}
	}

	let text = String::from_utf8_lossy(&body);
	bail!("canopy {what} failed ({status}): {text}")
}

#[cfg(test)]
mod tests {
	use age::secrecy::SecretString;
	use algae_cli::streams::encrypt_stream;
	use p256::{
		ecdsa::{VerifyingKey, signature::Verifier as _},
		elliptic_curve::{
			pkcs8::{DecodePublicKey as _, EncodePrivateKey as _, LineEnding},
			rand_core::OsRng,
		},
	};

	use super::*;

	const SAMPLE_TICKET: &str = r#"{"v":"enroll-1","api_url":"https://canopy.example","server_id":"7deb2793-0425-427e-8a19-7213946fa9be","token":"c2VjcmV0"}"#;

	fn test_key_pem() -> String {
		SecretKey::random(&mut OsRng)
			.to_pkcs8_pem(LineEnding::LF)
			.unwrap()
			.to_string()
	}

	#[test]
	fn transport_routes_and_spki_placement() {
		let api: Url = "https://canopy.example".parse().unwrap();

		let ts = Transport::Tailscale(reqwest::Client::new());
		assert_eq!(
			ts.url(&api, "begin").unwrap().as_str(),
			"https://canopy.tail53aef.ts.net/public/servers/register/begin"
		);
		assert!(ts.carries_spki_in_body());

		let mtls = Transport::Mtls(reqwest::Client::new());
		assert_eq!(
			mtls.url(&api, "complete").unwrap().as_str(),
			"https://canopy.example/servers/register/complete"
		);
		assert!(!mtls.carries_spki_in_body());
	}

	#[test]
	fn build_transcript_layout() {
		let nonce = [0xAAu8; 32];
		let server_id = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
		let spki = [0xBBu8; 91];
		let transcript = build_transcript(&nonce, &server_id, &spki);

		assert_eq!(transcript.len(), 32 + 16 + 91);
		assert_eq!(&transcript[..32], &nonce);
		assert_eq!(&transcript[32..48], server_id.as_bytes());
		assert_eq!(&transcript[48..], &spki);
	}

	#[tokio::test]
	async fn ticket_roundtrip_decrypts_and_parses() {
		let pass_phrase = SecretString::from("correct-horse-battery-staple");
		let recipient = Passphrase::new(pass_phrase.clone());

		let mut cursor = futures::io::Cursor::new(Vec::new());
		encrypt_stream(SAMPLE_TICKET.as_bytes(), &mut cursor, Box::new(recipient))
			.await
			.unwrap();
		let encrypted = cursor.into_inner();

		let ticket = decrypt_ticket(&encrypted, Passphrase::new(pass_phrase))
			.await
			.unwrap();
		assert_eq!(ticket.v, "enroll-1");
		assert_eq!(ticket.api_url, "https://canopy.example");
		assert_eq!(ticket.server_id, "7deb2793-0425-427e-8a19-7213946fa9be");
		assert_eq!(ticket.token, "c2VjcmV0");
	}

	#[tokio::test]
	async fn ticket_decrypt_fails_with_wrong_passphrase() {
		let recipient = Passphrase::new(SecretString::from("right-passphrase-here-please"));
		let mut cursor = futures::io::Cursor::new(Vec::new());
		encrypt_stream(SAMPLE_TICKET.as_bytes(), &mut cursor, Box::new(recipient))
			.await
			.unwrap();
		let encrypted = cursor.into_inner();

		let wrong = Passphrase::new(SecretString::from("wrong-passphrase-entirely-no"));
		assert!(decrypt_ticket(&encrypted, wrong).await.is_err());
	}

	#[test]
	fn signature_over_transcript_verifies_against_spki() {
		// The SPKI we put in the transcript must be the public half of the key
		// we sign with — otherwise Canopy's proof-of-possession check fails.
		let pem = test_key_pem();
		let spki = spki_der(&pem).unwrap();
		let signing_key = SigningKey::from_pkcs8_pem(&pem).unwrap();

		let nonce = [0x11u8; 32];
		let server_id = Uuid::from_u128(0xDEAD_BEEF);
		let transcript = build_transcript(&nonce, &server_id, &spki);

		let signature: Signature = signing_key.sign(&transcript);
		let der = signature.to_der();
		let parsed = Signature::from_der(der.as_bytes()).unwrap();

		let verifying = VerifyingKey::from_public_key_der(&spki).unwrap();
		verifying.verify(&transcript, &parsed).unwrap();
	}
}
