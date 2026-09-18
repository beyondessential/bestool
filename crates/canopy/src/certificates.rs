//! The keys and chains this host holds for canopy-issued TLS certificates.
//!
//! Canopy signs a certificate signing request and never sees the key behind it,
//! so generating the key, keeping it, and replacing it when canopy condemns it
//! are all this side's. Canopy keys what it holds by name *and* key, so a key
//! that did not survive a restart would turn every collection into a fresh
//! order.
//!
//! Two files, deliberately:
//!
//! - the keys, all of them, in one machine-bound encrypted file beside the
//!   registration ([`crate::machine_store`]) — nothing private at rest in
//!   plaintext, and unreadable on a different machine;
//! - each collected chain as a plain file in a directory alongside, a chain
//!   being public.
//!
//! That split is what keeps the encrypted payload small and static: a collection
//! that lands rewrites a plain file rather than re-encrypting the key store, so
//! keys are unlocked only when one is generated or read back after a restart.
//!
//! spec: TLS#keys

use std::{
	collections::BTreeMap,
	path::{Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use rcgen::{CertificateParams, CertificateSigningRequest, DistinguishedName};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tracing::debug;

use crate::machine_store::{
	decrypt_bytes, encrypt_bytes, machine_passphrase, remove_if_present, write_atomic,
};

/// The key type this module generates and hands back.
///
/// Re-exported so a consumer holding a key does not have to depend on rcgen
/// itself just to name it.
pub use rcgen::KeyPair;

const VERSION: &str = "certificate-keys-1";

/// Every name's private key, as held on this host.
///
/// One file for all of them: the payload stays small, and a name is added or
/// replaced by rewriting it whole. Keys are PKCS#8 PEM, as [`KeyPair`]
/// serialises them.
#[derive(Clone, Serialize, Deserialize)]
pub struct KeyStore {
	pub v: String,
	/// Name to PKCS#8 PEM. Ordered so the file's bytes don't churn on rewrite.
	#[serde(default)]
	keys: BTreeMap<String, String>,
}

impl Default for KeyStore {
	fn default() -> Self {
		Self {
			v: VERSION.to_owned(),
			keys: BTreeMap::new(),
		}
	}
}

impl std::fmt::Debug for KeyStore {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("KeyStore")
			.field("v", &self.v)
			.field("keys", &self.keys.keys().collect::<Vec<_>>())
			.finish()
	}
}

impl KeyStore {
	/// The names this store holds a key for.
	pub fn names(&self) -> impl Iterator<Item = &str> {
		self.keys.keys().map(String::as_str)
	}

	/// The key held for `name`, parsed.
	pub fn key(&self, name: &str) -> Option<Result<KeyPair>> {
		self.keys.get(name).map(|pem| {
			KeyPair::from_pem(pem)
				.into_diagnostic()
				.wrap_err_with(|| format!("parsing the stored key for {name}"))
		})
	}

	/// The key held for `name` as PEM, for handing to a consumer that wants it
	/// in that form — the delivery endpoint, which serves it beside the chain.
	pub fn key_pem(&self, name: &str) -> Option<&str> {
		self.keys.get(name).map(String::as_str)
	}

	/// Put a freshly generated key in place for `name`, replacing any before it.
	///
	/// Replacing is how a key canopy condemns is retired: the old one is gone
	/// from the store, so nothing can ask against it again.
	pub fn replace(&mut self, name: &str, key: &KeyPair) {
		self.keys
			.insert(name.to_owned(), key.serialize_pem())
			.inspect(|_| debug!(name, "replaced the key held for this name"));
	}

	/// Drop the key held for `name`, reporting whether there was one.
	pub fn forget(&mut self, name: &str) -> bool {
		self.keys.remove(name).is_some()
	}
}

/// Generate a key for one name.
///
/// ECDSA over P-256: what the device mTLS identity already uses, and what the
/// authority behind canopy issues against.
///
/// spec: TLS#keys
pub fn generate_key() -> Result<KeyPair> {
	KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
		.into_diagnostic()
		.wrap_err("generating a P-256 key pair")
}

/// Hex SHA-256 of a key's subject public key info.
///
/// This is how canopy names the key a certificate covers, so it is how the host
/// tells whether what canopy holds covers a key it still has.
pub fn key_fingerprint(key: &KeyPair) -> String {
	use rcgen::PublicKeyData as _;
	let digest = Sha256::digest(key.subject_public_key_info());
	digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Build the signing request canopy is asked to sign, base64-encoded DER.
///
/// Exactly one name, because canopy refuses a request carrying any other rather
/// than trimming it. The distinguished name is left empty: the subject
/// alternative name is what the authority issues against, and a common name
/// would be a second place for the name to disagree with itself.
///
/// spec: TLS#keys
pub fn signing_request(name: &str, key: &KeyPair) -> Result<String> {
	Ok(STANDARD.encode(csr_der(name, key)?))
}

fn csr_der(name: &str, key: &KeyPair) -> Result<Vec<u8>> {
	let mut params = CertificateParams::new(vec![name.to_owned()])
		.into_diagnostic()
		.wrap_err_with(|| format!("building signing request parameters for {name}"))?;
	params.distinguished_name = DistinguishedName::new();

	let csr: CertificateSigningRequest = params
		.serialize_request(key)
		.into_diagnostic()
		.wrap_err_with(|| format!("signing the request for {name}"))?;
	Ok(csr.der().to_vec())
}

/// Where the key store and the collected chains live.
///
/// Beside the registration, so all per-host canopy state shares one location and
/// one set of permissions.
pub fn default_dir() -> PathBuf {
	crate::machine_store::default_dir()
}

fn key_store_file(dir: &Path) -> PathBuf {
	dir.join("canopy-certificate-keys")
}

/// The directory collected chains sit in, one file per name.
pub fn chains_dir(dir: &Path) -> PathBuf {
	dir.join("canopy-certificates")
}

/// Read the key store from `dir`, or an empty one where there is no file yet.
///
/// A host that has never asked for a certificate has no store, which is not an
/// error: it is the state every host starts in.
pub async fn load_keys(dir: &Path) -> Result<KeyStore> {
	let path = key_store_file(dir);
	if !path.exists() {
		return Ok(KeyStore::default());
	}

	#[cfg(unix)]
	crate::machine_store::repair_mode(&path).await;

	let bytes = tokio::fs::read(&path)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("reading {}", path.display()))?;
	let plaintext = decrypt_bytes(&bytes, machine_passphrase()?).wrap_err(
		"decrypting the certificate key store (was this disk cloned from another machine?)",
	)?;
	serde_json::from_slice(&plaintext)
		.into_diagnostic()
		.wrap_err("parsing the certificate key store")
}

/// Write the key store to `dir`.
pub async fn store_keys(dir: &Path, keys: &KeyStore) -> Result<()> {
	tokio::fs::create_dir_all(dir)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("creating {}", dir.display()))?;
	let plaintext = serde_json::to_vec(keys)
		.into_diagnostic()
		.wrap_err("serialising the certificate key store")?;
	let ciphertext = encrypt_bytes(&plaintext, machine_passphrase()?)?;
	write_atomic(&key_store_file(dir), &ciphertext).await
}

/// The file a name's collected chain is kept in.
///
/// The name is sanitised into the file name rather than used raw: a name reaches
/// here from canopy's answer and from Caddy's configuration, and neither is a
/// path this side should be constructing from unchecked input. A wildcard's `*`
/// is written `_`, which is what makes `*.example.com` and `_.example.com`
/// collide — so the two are kept apart by hashing any name that isn't a plain
/// label sequence.
fn chain_file(dir: &Path, name: &str) -> PathBuf {
	chains_dir(dir).join(format!("{}.pem", chain_file_stem(name)))
}

fn chain_file_stem(name: &str) -> String {
	let plain = !name.is_empty()
		&& name.len() <= 200
		&& name
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
		&& !name.starts_with('.')
		&& !name.contains("..");
	if plain {
		return name.to_ascii_lowercase();
	}

	// Anything else — a wildcard, an internationalised name, something
	// unexpected — keeps a readable prefix for an operator reading the
	// directory and a digest to tell it apart from its neighbours.
	let digest = Sha256::digest(name.to_ascii_lowercase().as_bytes());
	let hex: String = digest
		.iter()
		.take(8)
		.map(|b| format!("{b:02x}"))
		.collect::<String>();
	let readable: String = name
		.to_ascii_lowercase()
		.chars()
		.map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
		.take(40)
		.collect();
	format!("{readable}-{hex}")
}

/// Read every chain this host has collected, keyed by the name it covers.
///
/// A file that can't be read is skipped rather than failing the lot: one
/// unreadable chain must not hide the others from a check that grades them.
pub async fn load_chains(dir: &Path) -> Result<BTreeMap<String, String>> {
	let names_path = chains_dir(dir);
	let mut out = BTreeMap::new();
	let mut entries = match tokio::fs::read_dir(&names_path).await {
		Ok(entries) => entries,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
		Err(err) => {
			return Err(err)
				.into_diagnostic()
				.wrap_err_with(|| format!("reading {}", names_path.display()));
		}
	};

	while let Some(entry) = entries.next_entry().await.into_diagnostic()? {
		let path = entry.path();
		if path.extension().is_none_or(|ext| ext != "pem") {
			continue;
		}
		match tokio::fs::read_to_string(&path).await {
			Ok(body) => match parse_chain_file(&body) {
				Some((name, chain)) => {
					out.insert(name, chain);
				}
				None => debug!(path = %path.display(), "chain file carries no name header"),
			},
			Err(err) => debug!(path = %path.display(), %err, "could not read a collected chain"),
		}
	}
	Ok(out)
}

/// Read one name's collected chain, or `None` where none has been.
pub async fn load_chain(dir: &Path, name: &str) -> Result<Option<String>> {
	let path = chain_file(dir, name);
	match tokio::fs::read_to_string(&path).await {
		Ok(body) => Ok(parse_chain_file(&body).map(|(_, chain)| chain)),
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
		Err(err) => Err(err)
			.into_diagnostic()
			.wrap_err_with(|| format!("reading {}", path.display())),
	}
}

/// Write a collected chain for `name`.
pub async fn store_chain(dir: &Path, name: &str, chain: &str) -> Result<()> {
	let chains = chains_dir(dir);
	tokio::fs::create_dir_all(&chains)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("creating {}", chains.display()))?;
	// The chains sit one level below the config directory, so the group has to
	// be carried down to them: a file inherits the group of the directory it is
	// in, and without this that is whatever group created this one.
	#[cfg(unix)]
	crate::machine_store::inherit_parent_group(&chains).await;

	let body = format!("{NAME_HEADER}{name}\n{chain}");
	write_atomic(&chain_file(dir, name), body.as_bytes()).await
}

/// Drop the chain held for `name`, reporting whether there was one.
pub async fn forget_chain(dir: &Path, name: &str) -> Result<bool> {
	remove_if_present(&chain_file(dir, name)).await
}

/// The name a chain covers, recorded in the file so reading the directory does
/// not have to reverse the file-name sanitising or parse the certificate.
const NAME_HEADER: &str = "# canopy-name: ";

fn parse_chain_file(body: &str) -> Option<(String, String)> {
	let mut lines = body.lines();
	let name = lines.next()?.strip_prefix(NAME_HEADER)?.trim().to_owned();
	if name.is_empty() {
		return None;
	}
	let rest = body
		.split_once('\n')
		.map(|(_, rest)| rest.to_owned())
		.unwrap_or_default();
	Some((name, rest))
}

/// Whether `name` sits at or beneath `domain`.
///
/// Canopy answers with the domains a group controls, and a name outside them is
/// not acted on. Case-insensitive, trailing dots ignored.
///
/// spec: NAM#entitlement
pub fn name_within(name: &str, domain: &str) -> bool {
	let name = name.trim_end_matches('.').to_ascii_lowercase();
	let domain = domain.trim_end_matches('.').to_ascii_lowercase();
	if domain.is_empty() {
		return false;
	}
	name == domain || name.ends_with(&format!(".{domain}"))
}

/// The expiry of a leaf in a PEM chain, as seconds since the epoch.
///
/// Reads the first certificate in the chain, which is the leaf.
pub fn chain_not_after(chain: &str) -> Option<i64> {
	let (_, pem) = x509_parser::pem::parse_x509_pem(chain.as_bytes()).ok()?;
	let cert = pem.parse_x509().ok()?;
	Some(cert.validity().not_after.timestamp())
}

/// The leaf certificate of a PEM chain, in DER.
pub fn chain_leaf_der(chain: &str) -> Option<Vec<u8>> {
	let (_, pem) = x509_parser::pem::parse_x509_pem(chain.as_bytes()).ok()?;
	Some(pem.contents)
}

/// The validity window of a chain's leaf, as seconds since the epoch.
pub fn chain_validity(chain: &str) -> Option<(i64, i64)> {
	let (_, pem) = x509_parser::pem::parse_x509_pem(chain.as_bytes()).ok()?;
	let cert = pem.parse_x509().ok()?;
	Some((
		cert.validity().not_before.timestamp(),
		cert.validity().not_after.timestamp(),
	))
}

/// Parse canopy's `not_after`, which arrives as an RFC 3339 timestamp.
pub fn parse_not_after(not_after: &str) -> Option<jiff::Timestamp> {
	not_after.parse().ok()
}

/// Reject a name that could not be a DNS name we would ask about.
///
/// The names this side acts on come from Caddy's configuration and from a
/// handshake's server name indication; neither is checked before it gets here.
pub fn plausible_name(name: &str) -> Result<()> {
	let trimmed = name.trim_end_matches('.');
	if trimmed.is_empty() || trimmed.len() > 253 {
		return Err(miette!("{name:?} is not a name a certificate can cover"));
	}
	if !trimmed.split('.').all(|label| {
		!label.is_empty()
			&& label.len() <= 63
			&& label
				.chars()
				.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '*')
	}) {
		return Err(miette!("{name:?} is not a name a certificate can cover"));
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_signing_request_carries_exactly_the_one_name() {
		// Canopy refuses a request naming anything other than the name asked
		// for, rather than trimming it, so the request this side builds has to
		// carry that one name and nothing else.
		use x509_parser::prelude::{FromDer as _, ParsedExtension};

		let key = generate_key().unwrap();
		let der = csr_der("app.example.com", &key).unwrap();
		let (_, csr) =
			x509_parser::certification_request::X509CertificationRequest::from_der(&der).unwrap();
		let names: Vec<String> = csr
			.requested_extensions()
			.expect("the request must ask for a name")
			.filter_map(|ext| match ext {
				ParsedExtension::SubjectAlternativeName(san) => Some(san),
				_ => None,
			})
			.flat_map(|san| san.general_names.iter())
			.filter_map(|gn| match gn {
				x509_parser::extensions::GeneralName::DNSName(dns) => Some((*dns).to_owned()),
				_ => None,
			})
			.collect();
		assert_eq!(names, vec!["app.example.com"]);
	}

	#[test]
	fn a_signing_request_carries_no_private_key() {
		// The key never leaves the machine: what goes on the wire is the
		// request, and the request is public.
		let key = generate_key().unwrap();
		let encoded = signing_request("app.example.com", &key).unwrap();
		let der = STANDARD.decode(&encoded).unwrap();
		let private = key.serialize_der();
		assert!(
			!der.windows(private.len()).any(|w| w == private),
			"the signing request must not carry the private key"
		);
	}

	#[test]
	fn a_fingerprint_follows_the_key_not_the_name() {
		let one = generate_key().unwrap();
		let two = generate_key().unwrap();
		assert_eq!(key_fingerprint(&one), key_fingerprint(&one));
		assert_ne!(key_fingerprint(&one), key_fingerprint(&two));
		assert_eq!(key_fingerprint(&one).len(), 64);
	}

	#[test]
	fn each_name_holds_its_own_key() {
		// Replacing one name's key is what a condemned key costs, and it must
		// cost only that name.
		let mut store = KeyStore::default();
		let first = generate_key().unwrap();
		let second = generate_key().unwrap();
		store.replace("a.example.com", &first);
		store.replace("b.example.com", &second);

		let replacement = generate_key().unwrap();
		store.replace("a.example.com", &replacement);

		let held_b = store.key("b.example.com").unwrap().unwrap();
		assert_eq!(key_fingerprint(&held_b), key_fingerprint(&second));
		let held_a = store.key("a.example.com").unwrap().unwrap();
		assert_eq!(key_fingerprint(&held_a), key_fingerprint(&replacement));
		assert_ne!(key_fingerprint(&held_a), key_fingerprint(&first));
	}

	#[tokio::test]
	async fn keys_survive_a_restart_and_are_not_at_rest_in_plaintext() {
		let dir = tempfile::tempdir().unwrap();
		let key = generate_key().unwrap();
		let mut store = KeyStore::default();
		store.replace("app.example.com", &key);
		store_keys(dir.path(), &store).await.unwrap();

		let back = load_keys(dir.path()).await.unwrap();
		let held = back.key("app.example.com").unwrap().unwrap();
		assert_eq!(key_fingerprint(&held), key_fingerprint(&key));

		let raw = std::fs::read(key_store_file(dir.path())).unwrap();
		assert!(
			!raw.windows(b"PRIVATE KEY".len())
				.any(|w| w == b"PRIVATE KEY"),
			"the key store must be encrypted at rest"
		);
	}

	#[tokio::test]
	async fn a_host_that_has_never_asked_has_an_empty_store() {
		let dir = tempfile::tempdir().unwrap();
		assert_eq!(load_keys(dir.path()).await.unwrap().names().count(), 0);
	}

	#[tokio::test]
	async fn chains_round_trip_by_name() {
		let dir = tempfile::tempdir().unwrap();
		store_chain(
			dir.path(),
			"app.example.com",
			"-----BEGIN CERTIFICATE-----\nx\n",
		)
		.await
		.unwrap();
		store_chain(
			dir.path(),
			"*.example.com",
			"-----BEGIN CERTIFICATE-----\ny\n",
		)
		.await
		.unwrap();

		let chains = load_chains(dir.path()).await.unwrap();
		assert_eq!(chains.len(), 2);
		assert!(chains["app.example.com"].contains('x'));
		assert!(chains["*.example.com"].contains('y'));

		assert!(
			load_chain(dir.path(), "app.example.com")
				.await
				.unwrap()
				.unwrap()
				.contains('x')
		);
		assert!(forget_chain(dir.path(), "app.example.com").await.unwrap());
		assert!(
			load_chain(dir.path(), "app.example.com")
				.await
				.unwrap()
				.is_none()
		);
	}

	/// The config directory is shared with unprivileged bestool invocations, so
	/// what the root daemon writes there has to carry the directory's group. The
	/// chains sit a level down, which is where that inheritance would otherwise
	/// stop.
	#[cfg(unix)]
	#[tokio::test]
	async fn a_collected_chain_carries_the_config_directory_group() {
		use std::os::unix::fs::MetadataExt as _;

		let dir = tempfile::tempdir().unwrap();
		let dir_gid = std::fs::metadata(dir.path()).unwrap().gid();

		// A group this process may chown to, other than the directory's own;
		// without a second group there is no mismatch to set up.
		let Some(shared) = std::process::Command::new("id")
			.arg("-G")
			.output()
			.ok()
			.and_then(|out| String::from_utf8(out.stdout).ok())
			.and_then(|groups| {
				groups
					.split_whitespace()
					.filter_map(|gid| gid.parse::<u32>().ok())
					.find(|gid| *gid != dir_gid)
			})
		else {
			return;
		};
		std::os::unix::fs::chown(dir.path(), None, Some(shared)).unwrap();

		store_chain(
			dir.path(),
			"app.example.com",
			"-----BEGIN CERTIFICATE-----\n",
		)
		.await
		.unwrap();

		let gid = std::fs::metadata(chain_file(dir.path(), "app.example.com"))
			.unwrap()
			.gid();
		assert_eq!(gid, shared, "expected group {shared}, got {gid}");
	}

	#[test]
	fn a_wildcard_does_not_collide_with_its_underscore_spelling() {
		assert_ne!(
			chain_file_stem("*.example.com"),
			chain_file_stem("_.example.com")
		);
	}

	#[test]
	fn a_chain_file_name_cannot_escape_its_directory() {
		for name in ["../../etc/shadow", "/etc/shadow", "..", "a/b"] {
			let stem = chain_file_stem(name);
			assert!(!stem.contains('/'), "{name:?} produced {stem:?}");
			assert!(!stem.contains(".."), "{name:?} produced {stem:?}");
		}
	}

	#[test]
	fn a_name_is_within_a_domain_at_or_beneath_it() {
		assert!(name_within("example.com", "example.com"));
		assert!(name_within("app.example.com", "example.com"));
		assert!(name_within("a.b.example.com", "example.com"));
		assert!(name_within("APP.Example.Com.", "example.com"));
		assert!(!name_within("example.com.evil.test", "example.com"));
		assert!(!name_within("notexample.com", "example.com"));
		assert!(!name_within("app.example.com", ""));
	}

	#[test]
	fn an_implausible_name_is_refused_before_it_is_asked_about() {
		assert!(plausible_name("app.example.com").is_ok());
		assert!(plausible_name("*.example.com").is_ok());
		assert!(plausible_name("").is_err());
		assert!(plausible_name("a..b").is_err());
		assert!(plausible_name("a/b.example.com").is_err());
		assert!(plausible_name(&"a".repeat(300)).is_err());
	}
}
