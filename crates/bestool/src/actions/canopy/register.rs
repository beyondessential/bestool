use std::{
	io::Read as _,
	path::{Path, PathBuf},
	time::Duration,
};

use algae_cli::{
	passphrases::{Passphrase, PassphraseArgs},
	streams::decrypt_stream,
};
use base64::{
	Engine as _,
	engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use bestool_canopy::{client_builder, device_identity};
use bestool_tamanu::server_info::{
	get_or_create_device_key_file, standard_device_key_path, standard_server_id_path,
};
use clap::Parser;
use miette::{IntoDiagnostic as _, Result, WrapErr as _, bail, miette};
use p256::{
	SecretKey,
	ecdsa::{Signature, SigningKey, signature::Signer as _},
	elliptic_curve::pkcs8::{DecodePrivateKey as _, EncodePublicKey as _},
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::actions::Context;

/// Enrol this machine as a Canopy server.
///
/// An operator first creates the server record in Canopy, which hands back an
/// encrypted enrollment ticket plus a separate passphrase (shared out of band).
/// This command decrypts the ticket, then claims the pre-created server over
/// mTLS by proving the machine holds the private key behind the certificate it
/// presents.
#[derive(Debug, Clone, Parser)]
pub struct RegisterArgs {
	/// Encrypted enrollment ticket from Canopy.
	///
	/// Copy-paste the whole `bestool canopy register <ticket>` line Canopy
	/// shows you. The ticket is encrypted, so it's safe to pass on the command
	/// line. If omitted, the ticket is read from stdin.
	pub ticket: Option<String>,

	/// Directory holding the machine's mTLS identity and Canopy state.
	///
	/// Defaults to the standard Tamanu config directory (`/etc/tamanu`, or
	/// `C:\Tamanu` on Windows). The device key, server-id, and registration
	/// record are read from and written under here.
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

/// On-disk record of a completed enrollment.
#[derive(Debug, Serialize, Deserialize)]
struct Registration {
	v: String,
	server_id: String,
	device_id: String,
	api_url: String,
}

#[derive(Serialize)]
struct BeginRequest<'a> {
	server_id: &'a str,
	token: &'a str,
}

#[derive(Deserialize)]
struct BeginResponse {
	nonce: String,
	#[serde(default)]
	channel_binding_required: bool,
}

#[derive(Serialize)]
struct CompleteRequest<'a> {
	server_id: &'a str,
	nonce: &'a str,
	signature: &'a str,
}

#[derive(Deserialize)]
struct CompleteResponse {
	server_id: String,
	device_id: String,
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

	let ticket_b64 = match ticket {
		Some(t) => t,
		None => read_ticket_from_stdin()?,
	};
	let encrypted = decode_ticket_base64(ticket_b64.trim())?;

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

	let paths = StatePaths::new(config.as_deref());

	// Idempotency: if we've already enrolled this server with our identity, the
	// token has been consumed and re-running would only fail opaquely. Treat a
	// matching local record as success.
	if let Some(existing) = read_registration(&paths.registration)
		&& existing.server_id == ticket.server_id
		&& !existing.device_id.is_empty()
	{
		info!(server_id = %existing.server_id, device_id = %existing.device_id, "already enrolled");
		println!("Already enrolled with Canopy.");
		println!("  server id: {}", existing.server_id);
		println!("  device id: {}", existing.device_id);
		return Ok(());
	}

	// Establish the machine's mTLS identity, reusing the device key if present.
	let device_key_pem = get_or_create_device_key_file(&paths.device_key)?;
	let identity = device_identity(&device_key_pem)?;
	let spki_der = spki_der(&device_key_pem)?;
	let signing_key = SigningKey::from_pkcs8_pem(&device_key_pem)
		.into_diagnostic()
		.wrap_err("loading device key for signing")?;

	let http = client_builder(env!("CARGO_PKG_VERSION"))
		.identity(identity)
		.use_rustls_tls()
		.timeout(Duration::from_secs(30))
		.build()
		.into_diagnostic()
		.wrap_err("building mTLS HTTP client")?;

	// Step 1: begin — fetch the challenge nonce. The token isn't consumed here.
	let begin = begin(&http, &api_url, &ticket.server_id, &ticket.token).await?;
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
		&http,
		&api_url,
		&ticket.server_id,
		&begin.nonce,
		&signature_b64,
	)
	.await?;

	// Persist the result so the agent knows it's bound and where to report.
	let registration = Registration {
		v: "registered-1".into(),
		server_id: complete.server_id.clone(),
		device_id: complete.device_id.clone(),
		api_url: api_url.to_string(),
	};
	write_registration(&paths.registration, &registration)?;
	persist_server_id(&paths.server_id, &complete.server_id)?;

	info!(server_id = %complete.server_id, device_id = %complete.device_id, "enrolled with canopy");
	println!("Enrolled with Canopy.");
	println!("  server id: {}", complete.server_id);
	println!("  device id: {}", complete.device_id);
	Ok(())
}

fn read_ticket_from_stdin() -> Result<String> {
	let mut buf = String::new();
	std::io::stdin()
		.read_to_string(&mut buf)
		.into_diagnostic()
		.wrap_err("reading ticket from stdin")?;
	if buf.trim().is_empty() {
		bail!("no ticket given on the command line or stdin");
	}
	Ok(buf)
}

/// Base64-decode the ticket, accepting every variant Canopy's lenient encoder
/// might produce (standard / no-pad / url-safe / url-safe-no-pad).
fn decode_ticket_base64(input: &str) -> Result<Vec<u8>> {
	for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
		if let Ok(bytes) = engine.decode(input) {
			return Ok(bytes);
		}
	}
	Err(miette!("ticket is not valid base64"))
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
	http: &reqwest::Client,
	api_url: &Url,
	server_id: &str,
	token: &str,
) -> Result<BeginResponse> {
	let url = api_url
		.join("/servers/register/begin")
		.into_diagnostic()
		.wrap_err("building register/begin URL")?;
	let resp = http
		.post(url)
		.json(&BeginRequest { server_id, token })
		.send()
		.await
		.into_diagnostic()
		.wrap_err("calling register/begin")?;
	parse_json_or_problem(resp, "register/begin").await
}

async fn complete(
	http: &reqwest::Client,
	api_url: &Url,
	server_id: &str,
	nonce: &str,
	signature: &str,
) -> Result<CompleteResponse> {
	let url = api_url
		.join("/servers/register/complete")
		.into_diagnostic()
		.wrap_err("building register/complete URL")?;
	let resp = http
		.post(url)
		.json(&CompleteRequest {
			server_id,
			nonce,
			signature,
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

/// Where the mTLS identity and Canopy state live on disk.
struct StatePaths {
	device_key: PathBuf,
	server_id: PathBuf,
	registration: PathBuf,
}

impl StatePaths {
	fn new(config_dir: Option<&Path>) -> Self {
		match config_dir {
			Some(dir) => Self {
				device_key: dir.join("device-key.pem"),
				server_id: dir.join("server-id"),
				registration: dir.join("canopy-registration.json"),
			},
			None => {
				let device_key = standard_device_key_path();
				let registration = device_key
					.parent()
					.unwrap_or_else(|| Path::new("."))
					.join("canopy-registration.json");
				Self {
					device_key,
					server_id: standard_server_id_path(),
					registration,
				}
			}
		}
	}
}

fn read_registration(path: &Path) -> Option<Registration> {
	let bytes = std::fs::read(path).ok()?;
	match serde_json::from_slice(&bytes) {
		Ok(reg) => Some(reg),
		Err(err) => {
			debug!(path = %path.display(), %err, "ignoring unreadable registration record");
			None
		}
	}
}

fn write_registration(path: &Path, reg: &Registration) -> Result<()> {
	let json = serde_json::to_vec_pretty(reg)
		.into_diagnostic()
		.wrap_err("serialising registration record")?;
	std::fs::write(path, json)
		.into_diagnostic()
		.wrap_err_with(|| format!("writing registration to {}", path.display()))
}

fn persist_server_id(path: &Path, id: &str) -> Result<()> {
	if let Ok(existing) = std::fs::read_to_string(path) {
		let existing = existing.trim();
		if !existing.is_empty() && existing != id {
			warn!(
				path = %path.display(),
				old = existing,
				new = id,
				"replacing existing server-id with the enrolled one"
			);
		}
	}
	std::fs::write(path, format!("{id}\n"))
		.into_diagnostic()
		.wrap_err_with(|| format!("writing server-id to {}", path.display()))
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
	fn decode_ticket_base64_accepts_all_variants() {
		let raw = b"\x00\xff\x10hello world?!";
		for encoded in [
			STANDARD.encode(raw),
			STANDARD_NO_PAD.encode(raw),
			URL_SAFE.encode(raw),
			URL_SAFE_NO_PAD.encode(raw),
		] {
			assert_eq!(decode_ticket_base64(&encoded).unwrap(), raw);
		}
	}

	#[test]
	fn decode_ticket_base64_rejects_garbage() {
		assert!(decode_ticket_base64("not valid base64 !!!! \u{00a0}").is_err());
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

	#[test]
	fn state_paths_default_share_a_directory() {
		let paths = StatePaths::new(None);
		assert_eq!(paths.device_key.parent(), paths.registration.parent());
		assert_eq!(paths.server_id.parent(), paths.registration.parent());
		assert_eq!(
			paths.registration.file_name().unwrap(),
			"canopy-registration.json"
		);
	}

	#[test]
	fn state_paths_override_directory() {
		let paths = StatePaths::new(Some(Path::new("/tmp/canopy-test")));
		assert_eq!(
			paths.device_key,
			Path::new("/tmp/canopy-test/device-key.pem")
		);
		assert_eq!(paths.server_id, Path::new("/tmp/canopy-test/server-id"));
		assert_eq!(
			paths.registration,
			Path::new("/tmp/canopy-test/canopy-registration.json")
		);
	}

	#[test]
	fn registration_roundtrips_through_disk() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("canopy-registration.json");
		let reg = Registration {
			v: "registered-1".into(),
			server_id: "7deb2793-0425-427e-8a19-7213946fa9be".into(),
			device_id: "11111111-2222-3333-4444-555555555555".into(),
			api_url: "https://canopy.example/".into(),
		};
		write_registration(&path, &reg).unwrap();

		let read = read_registration(&path).unwrap();
		assert_eq!(read.server_id, reg.server_id);
		assert_eq!(read.device_id, reg.device_id);
		assert_eq!(read.api_url, reg.api_url);
	}

	#[test]
	fn persist_server_id_writes_and_replaces() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("server-id");

		persist_server_id(&path, "first-id").unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "first-id");

		persist_server_id(&path, "second-id").unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap().trim(), "second-id");
	}
}
