//! Caddy TLS certificate expiry.
//!
//! Caddy auto-renews managed certs at roughly a third of their lifetime before
//! expiry, so a cert getting close to expiry means renewal is failing —
//! exactly the kind of slow-burn problem that's invisible until the site goes
//! down. The set of certs that matter comes from caddy's live admin config, not
//! the on-disk store — the store keeps certs for sites long since removed, and
//! alerting on those would be noise. We consider managed (ACME) certs whose
//! subjects are still active in the config, plus any manually-loaded certs the
//! config references; for each we also do a TLS handshake against the
//! locally-served endpoint to confirm caddy is serving what's configured.
//!
//! Two independent signals:
//!
//! - **Expiry** (from the on-disk cert): only evaluated once the cert is inside
//!   caddy's renewal window (it renews at ~1/3 of lifetime remaining), so we
//!   never alert before caddy would even have tried. Inside the window the
//!   thresholds scale with the cert's own lifetime, anchored on the 90-day case
//!   of warn at 21 days left / fail at 7. A 45-day cert warns ~10 days out, a
//!   6-day cert ~1.4 days out.
//! - **Served vs configured mismatch**: the handshake dials `127.0.0.1`
//!   directly (so the SNI resolves to the local caddy, never out to the
//!   internet) and compares the served leaf against the configured/on-disk
//!   leaf. A mismatch means caddy hasn't picked up a renewed cert (needs a
//!   reload) — a warning.
//!
//! Skips when caddy's admin config can't be read (e.g. not a caddy host).

use std::{
	collections::{BTreeSet, HashSet},
	path::{Path, PathBuf},
	sync::Arc,
	time::Duration,
};

use jiff::Timestamp;
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, net::TcpStream};
use tracing::debug;
use x509_parser::prelude::*;

use super::SweepContext;
use crate::doctor::Stat;
use crate::doctor::check::Check;

const NAME: &str = "caddy_certs";

/// Expiry thresholds as a fraction of each cert's total lifetime, anchored on
/// the 90-day case (warn 21d, fail 7d). Scaling keeps the same safety margin
/// for shorter-lived certs (e.g. 45-day or 6-day profiles).
const WARN_FRACTION: f64 = 21.0 / 90.0;
const FAIL_FRACTION: f64 = 7.0 / 90.0;

/// Caddy/certmagic's default renewal window: a managed cert is renewed once its
/// remaining lifetime drops below this fraction of the total (1/3, i.e. at
/// two-thirds elapsed). Before that point caddy hasn't even attempted renewal,
/// so a low remaining is expected and must not alert.
const RENEWAL_RATIO: f64 = 1.0 / 3.0;

const TLS_PORT: u16 = 443;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Sev {
	Warn,
	Fail,
}

#[derive(Debug, PartialEq, Eq)]
enum Expiry {
	Ok,
	Warn,
	Fail,
}

/// Classify a cert by remaining lifetime, but only once caddy would have
/// started renewing it. While `remaining` is still above the renewal window
/// (`RENEWAL_RATIO` of the total lifetime) caddy hasn't attempted renewal yet,
/// so even a modest remaining is normal and we stay `Ok`. Inside the window the
/// scaled warn/fail thresholds apply — a cert lingering there means renewal
/// isn't happening.
fn classify_expiry(remaining: i64, lifetime: i64) -> Expiry {
	let renewal_window = lifetime as f64 * RENEWAL_RATIO;
	if remaining as f64 >= renewal_window {
		return Expiry::Ok;
	}
	let warn_at = (lifetime as f64 * WARN_FRACTION) as i64;
	let fail_at = (lifetime as f64 * FAIL_FRACTION) as i64;
	if remaining <= fail_at {
		Expiry::Fail
	} else if remaining <= warn_at {
		Expiry::Warn
	} else {
		Expiry::Ok
	}
}

pub async fn run(ctx: SweepContext) -> Check {
	// The live admin config is the source of truth for which certs matter: the
	// on-disk store keeps certs for sites that have since been removed, and we
	// must not alert on those. So we read the config, then only consider managed
	// certs whose subjects are still active, plus any certs the config loads
	// manually.
	let Some(config) = fetch_admin_config(&ctx.http_client).await else {
		return Check::skip(
			NAME,
			"caddy admin config unavailable",
			"could not read the caddy admin API at localhost:2019",
		);
	};
	let active = active_subjects(&config);

	let mut certs: Vec<(String, DiskCert)> = Vec::new();
	if let Some(dir) = certificates_dir() {
		let mut files = Vec::new();
		collect_crt_files(&dir, &mut files);
		for path in files {
			let Ok(bytes) = std::fs::read(&path) else {
				continue;
			};
			let Some(cert) = parse_cert(&bytes) else {
				debug!(path = %path.display(), "could not parse caddy cert");
				continue;
			};
			if cert.covers_any(&active) {
				certs.push((path.display().to_string(), cert));
			} else {
				debug!(path = %path.display(), "managed cert not in active config; skipping");
			}
		}
	}
	for (origin, pem) in manual_sources(&config) {
		if let Some(cert) = parse_cert(&pem) {
			certs.push((origin, cert));
		}
	}

	if certs.is_empty() {
		return Check::skip(
			NAME,
			"no active caddy certificates",
			"the caddy config references no managed or manual certificates we could read",
		);
	}

	let now = Timestamp::now().as_second();
	let mut findings: Vec<(Sev, String)> = Vec::new();
	let mut details: Vec<Value> = Vec::new();
	let mut seen: HashSet<Vec<u8>> = HashSet::new();

	let mut stats: Vec<Stat> = Vec::new();
	for (origin, cert) in &certs {
		if !seen.insert(cert.der.clone()) {
			continue; // same cert reached via multiple subjects/sources
		}
		let label = if cert.sans.is_empty() {
			origin.clone()
		} else {
			cert.sans.join(", ")
		};

		// Expiry, scaled to the cert's own lifetime and gated on caddy's
		// renewal window (see `classify_expiry`).
		let lifetime = (cert.not_after - cert.not_before).max(1);
		let remaining = cert.not_after - now;
		let days = remaining as f64 / 86400.0;
		match classify_expiry(remaining, lifetime) {
			Expiry::Fail => findings.push((
				Sev::Fail,
				format!("{label}: expires in {days:.1}d (renewal is failing)"),
			)),
			Expiry::Warn => findings.push((Sev::Warn, format!("{label}: expires in {days:.1}d"))),
			Expiry::Ok => {}
		}

		// Served vs on-disk. Only meaningful for a concrete (non-wildcard) name.
		let mut served_matches: Option<bool> = None;
		if let Some(sni) = cert.sans.iter().find(|n| !n.contains('*')) {
			match served_leaf(sni).await {
				Ok(served_der) => {
					let matches = served_der == cert.der;
					served_matches = Some(matches);
					if !matches {
						findings.push((
							Sev::Warn,
							format!(
								"{label}: served cert differs from configured cert (caddy needs a reload?)"
							),
						));
					}
				}
				Err(e) => debug!(sni, error = %e, "could not read served cert"),
			}
		}

		let cert_id = cert.sans.first().cloned().unwrap_or_else(|| origin.clone());
		stats.push(
			Stat::gauge("days_remaining", (days * 10.0).round() / 10.0)
				.label("cert", cert_id.clone())
				.help("Days until certificate expiry"),
		);
		if let Some(matches) = served_matches {
			stats.push(
				Stat::gauge("served_matches", if matches { 1.0 } else { 0.0 })
					.label("cert", cert_id)
					.help("Served cert matches configured cert (1/0)"),
			);
		}

		details.push(json!({
			"names": cert.sans,
			"origin": origin,
			"not_after": cert.not_after,
			"days_remaining": (days * 10.0).round() / 10.0,
			"lifetime_days": lifetime / 86400,
			"served_matches": served_matches,
		}));
	}

	let worst = findings.iter().map(|(s, _)| *s).max();
	let reasons = findings
		.iter()
		.map(|(_, m)| m.as_str())
		.collect::<Vec<_>>()
		.join("; ");
	let n = details.len();
	let check = match worst {
		Some(Sev::Fail) => Check::fail(NAME, format!("{n} cert(s) checked"), reasons),
		Some(Sev::Warn) => Check::warning(NAME, format!("{n} cert(s) checked"), reasons),
		None => Check::pass(NAME, format!("{n} cert(s) valid")),
	};
	check
		.with_detail("certificates", Value::Array(details))
		.with_stat(Stat::gauge("count", n as f64).help("Certificates checked"))
		.with_stats(stats)
}

/// caddy's `certificates/` store, probed at the well-known data-dir locations
/// (the data dir belongs to the caddy service user, not ours, so we can't just
/// ask `dirs`). caddy's layout is `<data_dir>/certificates`, and `<data_dir>`
/// is `$XDG_DATA_HOME/caddy` when XDG is set — hence the extra `caddy/`
/// candidate under each root. `BESTOOL_CADDY_DATA_DIR` overrides for anything
/// non-standard.
fn certificates_dir() -> Option<PathBuf> {
	let mut roots: Vec<PathBuf> = Vec::new();
	if let Some(dir) = std::env::var_os("BESTOOL_CADDY_DATA_DIR") {
		roots.push(PathBuf::from(dir));
	}

	#[cfg(windows)]
	{
		// BES installs caddy at C:\Caddy; cover both a data dir set directly
		// there and the XDG-style `caddy` sub-dir, plus caddy's Windows default
		// of %AppData%\Caddy.
		roots.push(PathBuf::from(r"C:\Caddy"));
		roots.push(PathBuf::from(r"C:\Caddy\data"));
		if let Some(appdata) = std::env::var_os("APPDATA") {
			roots.push(PathBuf::from(appdata).join("Caddy"));
		}
	}
	#[cfg(not(windows))]
	{
		roots.push(PathBuf::from("/var/lib/caddy/.local/share/caddy"));
		roots.push(PathBuf::from("/var/lib/caddy"));
	}

	roots.into_iter().find_map(|root| {
		["certificates", "caddy/certificates"]
			.into_iter()
			.map(|sub| root.join(sub))
			.find(|dir| dir.is_dir())
	})
}

fn collect_crt_files(dir: &Path, out: &mut Vec<PathBuf>) {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return;
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if path.is_dir() {
			collect_crt_files(&path, out);
		} else if path.extension().is_some_and(|e| e == "crt") {
			out.push(path);
		}
	}
}

struct DiskCert {
	sans: Vec<String>,
	not_before: i64,
	not_after: i64,
	der: Vec<u8>,
}

impl DiskCert {
	/// Whether any of this cert's SANs is one of the active subjects, treating
	/// wildcard SANs (and wildcard subjects) appropriately.
	fn covers_any(&self, active: &BTreeSet<String>) -> bool {
		self.sans
			.iter()
			.any(|san| active.iter().any(|subj| name_matches(san, subj)))
	}
}

/// Parse a leaf certificate from PEM bytes (the first cert block).
fn parse_cert(pem: &[u8]) -> Option<DiskCert> {
	let (_, pem) = parse_x509_pem(pem).ok()?;
	let cert = pem.parse_x509().ok()?;
	let sans = cert
		.subject_alternative_name()
		.ok()
		.flatten()
		.map(|san| {
			san.value
				.general_names
				.iter()
				.filter_map(|gn| match gn {
					GeneralName::DNSName(d) => Some((*d).to_string()),
					_ => None,
				})
				.collect()
		})
		.unwrap_or_default();
	Some(DiskCert {
		sans,
		not_before: cert.validity().not_before.timestamp(),
		not_after: cert.validity().not_after.timestamp(),
		der: pem.contents,
	})
}

/// Fetch caddy's live config from the local admin API. `None` on any error
/// (admin API disabled, unreachable, or non-2xx) — the caller then skips, since
/// without the config it can't tell which certs are still in use.
async fn fetch_admin_config(client: &reqwest::Client) -> Option<serde_json::Value> {
	let resp = client
		.get("http://localhost:2019/config/")
		.timeout(Duration::from_secs(3))
		.send()
		.await
		.ok()?;
	if !resp.status().is_success() {
		return None;
	}
	resp.json().await.ok()
}

/// Every hostname caddy considers active, gathered from the config: route
/// `host` matchers (anywhere, including subroutes), plus TLS `automate` and
/// automation-policy `subjects`. Lower-cased for case-insensitive matching.
fn active_subjects(config: &serde_json::Value) -> BTreeSet<String> {
	fn walk(value: &serde_json::Value, out: &mut BTreeSet<String>) {
		match value {
			serde_json::Value::Object(map) => {
				for (key, val) in map {
					if matches!(key.as_str(), "host" | "automate" | "subjects")
						&& let Some(arr) = val.as_array()
					{
						out.extend(
							arr.iter()
								.filter_map(|v| v.as_str())
								.map(|s| s.to_ascii_lowercase()),
						);
					}
					walk(val, out);
				}
			}
			serde_json::Value::Array(arr) => arr.iter().for_each(|v| walk(v, out)),
			_ => {}
		}
	}
	let mut out = BTreeSet::new();
	walk(config, &mut out);
	out
}

/// Certificates the config loads explicitly (manually-issued, not ACME): the
/// `apps.tls.certificates` `load_files` paths and inline `load_pem` blobs.
/// Returns `(origin label, PEM bytes)` pairs.
fn manual_sources(config: &serde_json::Value) -> Vec<(String, Vec<u8>)> {
	let mut out = Vec::new();
	let certs = &config["apps"]["tls"]["certificates"];
	if let Some(files) = certs["load_files"].as_array() {
		for file in files {
			if let Some(path) = file["certificate"].as_str()
				&& let Ok(bytes) = std::fs::read(path)
			{
				out.push((path.to_string(), bytes));
			}
		}
	}
	if let Some(pems) = certs["load_pem"].as_array() {
		for (i, entry) in pems.iter().enumerate() {
			if let Some(pem) = entry["certificate"].as_str() {
				out.push((
					format!("caddy config load_pem[{i}]"),
					pem.as_bytes().to_vec(),
				));
			}
		}
	}
	out
}

/// Hostname match treating either side as a possible single-label wildcard
/// (`*.example.com`). Case-insensitive; trailing dots ignored.
fn name_matches(a: &str, b: &str) -> bool {
	let a = a.trim_end_matches('.').to_ascii_lowercase();
	let b = b.trim_end_matches('.').to_ascii_lowercase();
	a == b || wildcard_covers(&a, &b) || wildcard_covers(&b, &a)
}

/// Whether `pattern` (`*.example.com`) covers `name` (`host.example.com`) —
/// exactly one extra label, per RFC 6125 wildcard rules.
fn wildcard_covers(pattern: &str, name: &str) -> bool {
	let Some(base) = pattern.strip_prefix("*.") else {
		return false;
	};
	let Some(rest) = name.strip_suffix(base) else {
		return false;
	};
	let label = rest.strip_suffix('.').unwrap_or_default();
	!label.is_empty() && !label.contains('.')
}

/// TLS-handshake `127.0.0.1:443` with the given SNI and return the served leaf
/// cert's DER. Dialing the loopback address directly is the "DNS override": the
/// SNI selects the vhost but the connection never leaves the box. We accept any
/// cert — we only want to read it, not trust it.
async fn served_leaf(sni: &str) -> Result<Vec<u8>, String> {
	use rustls::{
		ClientConfig, DigitallySignedStruct, SignatureScheme,
		client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
		crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
		pki_types::{CertificateDer, ServerName, UnixTime},
	};
	use tokio_rustls::TlsConnector;

	#[derive(Debug)]
	struct AcceptAny(Arc<CryptoProvider>);
	impl ServerCertVerifier for AcceptAny {
		fn verify_server_cert(
			&self,
			_end_entity: &CertificateDer<'_>,
			_intermediates: &[CertificateDer<'_>],
			_server_name: &ServerName<'_>,
			_ocsp: &[u8],
			_now: UnixTime,
		) -> Result<ServerCertVerified, rustls::Error> {
			Ok(ServerCertVerified::assertion())
		}
		fn verify_tls12_signature(
			&self,
			message: &[u8],
			cert: &CertificateDer<'_>,
			dss: &DigitallySignedStruct,
		) -> Result<HandshakeSignatureValid, rustls::Error> {
			verify_tls12_signature(
				message,
				cert,
				dss,
				&self.0.signature_verification_algorithms,
			)
		}
		fn verify_tls13_signature(
			&self,
			message: &[u8],
			cert: &CertificateDer<'_>,
			dss: &DigitallySignedStruct,
		) -> Result<HandshakeSignatureValid, rustls::Error> {
			verify_tls13_signature(
				message,
				cert,
				dss,
				&self.0.signature_verification_algorithms,
			)
		}
		fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
			self.0.signature_verification_algorithms.supported_schemes()
		}
	}

	let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
	let config = ClientConfig::builder_with_provider(provider.clone())
		.with_safe_default_protocol_versions()
		.map_err(|e| e.to_string())?
		.dangerous()
		.with_custom_certificate_verifier(Arc::new(AcceptAny(provider)))
		.with_no_client_auth();
	let connector = TlsConnector::from(Arc::new(config));
	let server_name = ServerName::try_from(sni.to_string()).map_err(|e| e.to_string())?;

	let handshake = async {
		let tcp = TcpStream::connect(("127.0.0.1", TLS_PORT))
			.await
			.map_err(|e| e.to_string())?;
		let mut tls = connector
			.connect(server_name, tcp)
			.await
			.map_err(|e| e.to_string())?;
		let der = tls
			.get_ref()
			.1
			.peer_certificates()
			.and_then(|c| c.first())
			.map(|c| c.as_ref().to_vec())
			.ok_or_else(|| "no peer certificate".to_string())?;
		// Be polite and close the connection rather than leaving caddy hanging.
		let _ = tls.shutdown().await;
		Ok::<_, String>(der)
	};

	match tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake).await {
		Ok(r) => r,
		Err(_) => Err("handshake timed out".to_string()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn classify(remaining: i64, lifetime: i64) -> &'static str {
		match classify_expiry(remaining, lifetime) {
			Expiry::Ok => "pass",
			Expiry::Warn => "warn",
			Expiry::Fail => "fail",
		}
	}

	const D: i64 = 86400;

	#[test]
	fn ninety_day_cert_bands() {
		let life = 90 * D;
		assert_eq!(classify(40 * D, life), "pass");
		assert_eq!(classify(20 * D, life), "warn"); // <21d
		assert_eq!(classify(6 * D, life), "fail"); // <7d
		assert_eq!(classify(-1, life), "fail"); // expired
	}

	#[test]
	fn short_lived_certs_scale() {
		// 45-day cert: warn ~10.5d, fail ~3.5d.
		let life = 45 * D;
		assert_eq!(classify(20 * D, life), "pass");
		assert_eq!(classify(9 * D, life), "warn");
		assert_eq!(classify(2 * D, life), "fail");

		// 6-day cert: warn ~1.4d, fail ~0.47d.
		let life = 6 * D;
		assert_eq!(classify(3 * D, life), "pass");
		assert_eq!(classify(D, life), "warn");
		assert_eq!(classify(D / 4, life), "fail");
	}

	#[test]
	fn manually_issued_long_lived_certs() {
		// Operator-installed certs that aren't ACME-managed can span months or a
		// year. Thresholds scale to the long lifetime, and the renewal-window
		// gate still keeps quiet until ~1/3 remains — giving plenty of human
		// lead time to reissue without nagging a year out.

		// 1-year cert: window ~122d, warn ~85d, fail ~28d.
		let life = 365 * D;
		assert_eq!(classify(200 * D, life), "pass"); // far out
		assert_eq!(classify(130 * D, life), "pass"); // still before the window
		assert_eq!(classify(100 * D, life), "pass"); // in window, above warn
		assert_eq!(classify(80 * D, life), "warn"); // <~85d
		assert_eq!(classify(20 * D, life), "fail"); // <~28d

		// 200-day cert: window ~67d, warn ~47d, fail ~16d.
		let life = 200 * D;
		assert_eq!(classify(100 * D, life), "pass"); // before window
		assert_eq!(classify(60 * D, life), "pass"); // in window, above warn
		assert_eq!(classify(40 * D, life), "warn"); // <~47d
		assert_eq!(classify(10 * D, life), "fail"); // <~16d
	}

	#[test]
	fn no_alert_before_renewal_window() {
		// Caddy renews at 1/3 remaining (30d for a 90d cert); anything above
		// that is normal and must stay OK regardless of the warn threshold.
		let life = 90 * D;
		assert_eq!(classify(60 * D, life), "pass"); // fresh
		assert_eq!(classify(31 * D, life), "pass"); // just before the window
		// Even if the warn threshold were raised past the renewal window, the
		// gate keeps us quiet until caddy has had its chance to renew.
		assert_eq!(classify_expiry(40 * D, life), Expiry::Ok);
	}

	#[test]
	fn name_matching_handles_wildcards() {
		assert!(name_matches("app.example.com", "app.example.com"));
		assert!(name_matches("APP.example.com", "app.example.com")); // case-insensitive
		assert!(name_matches("*.example.com", "app.example.com")); // wildcard SAN covers host
		assert!(name_matches("app.example.com", "*.example.com")); // wildcard subject
		assert!(!name_matches("*.example.com", "a.b.example.com")); // only one label
		assert!(!name_matches("*.example.com", "example.com")); // bare apex not covered
		assert!(!name_matches("app.example.com", "app.example.org"));
	}

	#[test]
	fn active_subjects_pulls_hosts_automate_and_subjects() {
		let config = serde_json::json!({
			"apps": {
				"http": { "servers": { "srv0": { "routes": [
					{ "match": [{ "host": ["a.example.com", "b.example.com"] }],
					  "handle": [{ "handler": "subroute", "routes": [
						{ "match": [{ "host": ["nested.example.com"] }] }
					  ]}] }
				]}}},
				"tls": {
					"certificates": { "automate": ["auto.example.com"] },
					"automation": { "policies": [{ "subjects": ["policy.example.com"] }] }
				}
			}
		});
		let subjects = active_subjects(&config);
		for host in [
			"a.example.com",
			"b.example.com",
			"nested.example.com",
			"auto.example.com",
			"policy.example.com",
		] {
			assert!(subjects.contains(host), "missing {host}");
		}
	}

	#[test]
	fn manual_sources_reads_inline_pem() {
		let config = serde_json::json!({
			"apps": { "tls": { "certificates": { "load_pem": [
				{ "certificate": "-----BEGIN CERTIFICATE-----\nINLINE\n-----END CERTIFICATE-----", "key": "..." }
			]}}}
		});
		let sources = manual_sources(&config);
		assert_eq!(sources.len(), 1);
		assert_eq!(sources[0].0, "caddy config load_pem[0]");
		assert!(sources[0].1.starts_with(b"-----BEGIN CERTIFICATE-----"));
	}
}
