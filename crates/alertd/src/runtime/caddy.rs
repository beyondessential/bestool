//! Reading the traffic a Tamanu deployment serves, through the Caddy in front
//! of it on this machine.
//!
//! Caddy's admin API at `localhost:2019` exposes `/metrics` in Prometheus text
//! format, and the relevant series is the request-duration histogram's count,
//! labelled with the HTTP status code.
//!
//! One local Caddy fronts the whole machine, so it is one source. Where traffic
//! is served by shared infrastructure instead, a substrate reads that
//! infrastructure's counts and filters them to the application being reported
//! for — the reason the counters come back per source rather than as one total.
//!
//! Caddy instruments its handlers only when its config switches metrics on, and
//! serves `/metrics` with its process and admin series either way. So finding no
//! request counters at all says nothing about how busy the server is, and this
//! reads the config to tell an idle server from an uninstrumented one: an
//! uninstrumented Caddy is a reading that cannot be taken, not a quiet one.
//!
//! spec: SUB#http-traffic-and-certificates

use std::{
	collections::{BTreeMap, BTreeSet, HashSet, btree_map::Entry},
	path::{Path, PathBuf},
	sync::Arc,
	time::Duration,
};

use async_trait::async_trait;
use jiff::Timestamp;
use serde_json::Value;
use tokio::{io::AsyncWriteExt, net::TcpStream};
use tracing::debug;
use x509_parser::prelude::*;

use super::{Certificate, HttpRuntime, TrafficCounters, TrafficSource, Unavailable};
use crate::checks::fmt_chain;

const METRICS_URL: &str = "http://localhost:2019/metrics";
const CONFIG_URL: &str = "http://localhost:2019/config/apps/http";
const TIMEOUT: Duration = Duration::from_secs(3);

/// What the one Caddy on this machine is named as a source of counts.
///
/// A machine has one, and it fronts everything on it, so the name is fixed. It
/// exists at all because a substrate whose counts come from several front ends
/// keeps history per source, and a source that vanishes must be dropped rather
/// than read as the quantity having fallen.
const SOURCE: &str = "caddy";

/// Caddy serves TLS on the standard port, and the handshake dials the loopback
/// so the SNI selects the vhost without the connection leaving the box.
const TLS_PORT: u16 = 443;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// The Caddy serving a deployment on the machine this process is on.
pub struct CaddyRuntime {
	/// Shared with the checks so TCP connections stay warm between ticks.
	http: reqwest::Client,
}

impl CaddyRuntime {
	pub fn new(http: reqwest::Client) -> Self {
		Self { http }
	}

	/// Caddy's own view of whether it counts requests.
	///
	/// Only asked when no counters came back at all: that is either a quiet
	/// server or one not counting, and the two are a reading and the absence of
	/// one.
	async fn instrumented(&self) -> Result<bool, Unavailable> {
		let config = match self.http.get(CONFIG_URL).timeout(TIMEOUT).send().await {
			Ok(resp) if resp.status().is_success() => resp.json::<Value>().await,
			Ok(resp) => {
				debug!(status = %resp.status(), "caddy config endpoint refused");
				return Err(Unavailable::new(format!(
					"caddy reported no requests at all, and its config at {CONFIG_URL} answered HTTP {} — whether it counts requests could not be established",
					resp.status().as_u16()
				)));
			}
			Err(err) => {
				return Err(Unavailable::new(format!(
					"caddy reported no requests at all, and its config at {CONFIG_URL} could not be read ({}) — whether it counts requests could not be established",
					fmt_chain(&err)
				)));
			}
		};

		match config {
			Ok(config) => Ok(metrics_enabled_in(&config)),
			Err(err) => Err(Unavailable::new(format!(
				"caddy reported no requests at all, and its config at {CONFIG_URL} did not parse ({}) — whether it counts requests could not be established",
				fmt_chain(&err)
			))),
		}
	}
}

#[async_trait]
impl HttpRuntime for CaddyRuntime {
	async fn http_counters(&self) -> Result<TrafficCounters, Unavailable> {
		let body = match self.http.get(METRICS_URL).timeout(TIMEOUT).send().await {
			Ok(resp) if resp.status().is_success() => resp.text().await.map_err(|err| {
				Unavailable::new(format!(
					"caddy /metrics body read failed: {}",
					fmt_chain(&err)
				))
			})?,
			Ok(resp) => {
				let status = resp.status().as_u16();
				return Err(Unavailable::new(format!(
					"caddy is reachable but its admin /metrics endpoint isn't usable (HTTP {status}) — error rate cannot be measured"
				)));
			}
			Err(err) => {
				return Err(Unavailable::new(format!(
					"could not reach caddy admin at {METRICS_URL}: {}",
					fmt_chain(&err)
				)));
			}
		};

		let by_status = parse_status_counts(&body);
		if by_status.is_empty() && !self.instrumented().await? {
			return Err(Unavailable::new(
				"caddy counts requests only when its config asks it to, so there is no error rate to grade. Add `metrics` to the Caddyfile's global options — within a `servers` block on caddy older than 2.9.",
			));
		}

		Ok(TrafficCounters {
			sources: vec![TrafficSource {
				source: SOURCE.to_string(),
				by_status,
			}],
		})
	}

	async fn certificates(&self) -> Result<Vec<Certificate>, Unavailable> {
		read_certificates(&self.http).await
	}
}

/// The certificates in force for the application, as Caddy holds them.
///
/// The live admin config is the authority on which certificates matter: the
/// on-disk store keeps certificates for sites since removed, and reporting
/// those would have a check alert on something nothing serves. So the config is
/// read first, then only managed certificates whose subjects are still active,
/// plus any the config loads by hand.
async fn read_certificates(client: &reqwest::Client) -> Result<Vec<Certificate>, Unavailable> {
	let Some(config) = fetch_admin_config(client).await else {
		return Err(Unavailable::new(
			"could not read the caddy admin API at localhost:2019",
		));
	};
	let active = active_subjects(&config);

	// Reading the on-disk cert store (a recursive directory walk plus a read per
	// cert) and any manually-loaded cert files is blocking I/O — on Windows each
	// read is antivirus-scanned. Run it on the blocking pool so it can't stall
	// other checks sharing the executor.
	let found = tokio::task::spawn_blocking(move || gather_certs(&active, &config))
		.await
		.unwrap_or_else(|err| {
			debug!(%err, "caddy certificate scan task did not complete");
			Vec::new()
		});

	let mut out: Vec<Certificate> = Vec::new();
	let mut seen: HashSet<Vec<u8>> = HashSet::new();
	for (origin, cert) in found {
		// The same certificate is reachable through several subjects and
		// sources; a substrate reports each one once.
		if !seen.insert(cert.der.clone()) {
			continue;
		}
		let (Ok(not_before), Ok(not_after)) = (
			Timestamp::from_second(cert.not_before),
			Timestamp::from_second(cert.not_after),
		) else {
			debug!(
				origin,
				"certificate has validity dates outside the representable range"
			);
			continue;
		};

		let served_leaf = match name_to_dial(&cert.sans) {
			Some(sni) => match served_leaf(sni).await {
				Ok(der) => Some(der),
				Err(err) => {
					debug!(sni, %err, "could not read the served certificate");
					None
				}
			},
			None => None,
		};

		out.push(Certificate {
			names: cert.sans,
			origin,
			not_before,
			not_after,
			leaf: cert.der,
			served_leaf,
		});
	}

	Ok(out)
}

/// Which of a certificate's names to take a handshake against.
///
/// A wildcard names no host to dial, so a certificate that is only wildcards
/// has nothing to compare against. That is an absence of a reading, not a
/// mismatch, and the check must not grade it as one.
fn name_to_dial(names: &[String]) -> Option<&String> {
	names.iter().find(|name| !name.contains('*'))
}

/// Whether caddy's http app config switches request metrics on. Caddy 2.9 moved
/// the switch onto the app itself; before that each server carried its own, and
/// one instrumented server is enough to produce counters.
fn metrics_enabled_in(http_app: &Value) -> bool {
	if !http_app["metrics"].is_null() {
		return true;
	}
	http_app["servers"]
		.as_object()
		.is_some_and(|servers| servers.values().any(|server| !server["metrics"].is_null()))
}

/// Parse `caddy_http_request_duration_seconds_count{code="NNN",...} <count>` lines.
///
/// Caddy emits this histogram-count series labelled by `code`, `handler`,
/// `host`, `method`, `server`. The same request is observed by every handler
/// in the chain (encode, headers, rate_limit, reverse_proxy, …), so a naive
/// sum across labels would multiply the real request count by the depth of
/// the handler chain. To dedupe, we group by `(host, method, server, code)`
/// and take the **max** across handlers: the entry-point handler must have
/// seen every request matching that label combination, so its count is the
/// real one. Then we sum across hosts/methods/servers per code.
fn parse_status_counts(body: &str) -> BTreeMap<String, u64> {
	use std::collections::HashMap;

	let mut per_tuple: HashMap<(String, String, String, String), u64> = HashMap::new();
	for line in body.lines() {
		if line.starts_with('#') {
			continue;
		}
		let Some(rest) = line.strip_prefix("caddy_http_request_duration_seconds_count") else {
			continue;
		};
		let Some(labels_end) = rest.find('}') else {
			continue;
		};
		let labels = &rest[..labels_end];
		let value_part = rest[labels_end + 1..].trim();
		let value: u64 = match value_part.split_whitespace().next() {
			Some(v) => match v.parse::<f64>() {
				Ok(f) => f as u64,
				Err(_) => continue,
			},
			None => continue,
		};
		let Some(code) = extract_label(labels, "code") else {
			continue;
		};
		let host = extract_label(labels, "host").unwrap_or_default();
		let method = extract_label(labels, "method").unwrap_or_default();
		let server = extract_label(labels, "server").unwrap_or_default();
		let key = (host, method, server, code);
		let entry = per_tuple.entry(key).or_insert(0);
		*entry = (*entry).max(value);
	}

	let mut totals: BTreeMap<String, u64> = BTreeMap::new();
	for ((_, _, _, code), count) in per_tuple {
		*totals.entry(code).or_insert(0) += count;
	}
	totals
}

fn extract_label(labels: &str, key: &str) -> Option<String> {
	let needle = format!("{key}=\"");
	let start = labels.find(&needle)? + needle.len();
	let rest = &labels[start..];
	let end = rest.find('"')?;
	Some(rest[..end].to_string())
}

/// Read the caddy cert store and any manually-loaded cert files from disk,
/// reducing the store to the one live cert per set of names. All blocking I/O,
/// so it runs under `spawn_blocking`.
fn gather_certs(active: &BTreeSet<String>, config: &Value) -> Vec<(String, DiskCert)> {
	let mut managed: Vec<(String, DiskCert)> = Vec::new();
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
			if cert.covers_any(active) {
				managed.push((path.display().to_string(), cert));
			} else {
				debug!(path = %path.display(), "managed cert not in active config; skipping");
			}
		}
	}
	let mut certs = keep_live_certs(managed);
	for (origin, pem) in manual_sources(config) {
		if let Some(cert) = parse_cert(&pem) {
			certs.push((origin, cert));
		}
	}
	certs
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

	/// The set of names this cert covers, in a canonical form so that two certs
	/// issued for the same names key alike.
	fn name_key(&self) -> Vec<String> {
		let mut names: Vec<String> = self
			.sans
			.iter()
			.map(|san| san.trim_end_matches('.').to_ascii_lowercase())
			.collect();
		names.sort_unstable();
		names.dedup();
		names
	}
}

/// Reduce the certs found in caddy's store to the one live cert per set of
/// names: the one that expires last. A renewal issued by a different CA than the
/// previous one is written to that CA's own directory, leaving the older copy in
/// place under the CA that has been dropped, where it ages out and expires
/// without anyone renewing it. Grading it would report an expiry failure and a
/// mismatch against what's served for a file caddy stopped using.
///
/// Only applies to the store: a cert the config loads by path is one the
/// operator maintains, and each such cert is graded on its own.
///
/// A cert with no DNS names can't be grouped by name, so those are kept as they
/// are.
fn keep_live_certs(certs: Vec<(String, DiskCert)>) -> Vec<(String, DiskCert)> {
	let mut newest: BTreeMap<Vec<String>, (String, DiskCert)> = BTreeMap::new();
	let mut unnamed: Vec<(String, DiskCert)> = Vec::new();
	for (origin, cert) in certs {
		let key = cert.name_key();
		if key.is_empty() {
			unnamed.push((origin, cert));
			continue;
		}
		match newest.entry(key) {
			Entry::Occupied(mut slot) => {
				if cert.not_after > slot.get().1.not_after {
					debug!(
						superseded = %slot.get().0,
						live = %origin,
						"ignoring cert superseded by a later issuance"
					);
					slot.insert((origin, cert));
				} else {
					debug!(superseded = %origin, live = %slot.get().0, "ignoring cert superseded by a later issuance");
				}
			}
			Entry::Vacant(slot) => {
				slot.insert((origin, cert));
			}
		}
	}
	newest.into_values().chain(unnamed).collect()
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

	const D: i64 = 86400;

	const SAMPLE: &str = "\
# HELP caddy_http_request_duration_seconds Histogram of round-trip request durations.
# TYPE caddy_http_request_duration_seconds histogram
caddy_http_request_duration_seconds_count{code=\"200\",handler=\"encode\",host=\"a\",method=\"GET\",server=\"srv0\"} 3
caddy_http_request_duration_seconds_count{code=\"200\",handler=\"headers\",host=\"a\",method=\"GET\",server=\"srv0\"} 9
caddy_http_request_duration_seconds_count{code=\"200\",handler=\"rate_limit\",host=\"a\",method=\"GET\",server=\"srv0\"} 3
caddy_http_request_duration_seconds_count{code=\"200\",handler=\"reverse_proxy\",host=\"a\",method=\"GET\",server=\"srv0\"} 3
caddy_http_request_duration_seconds_count{code=\"404\",handler=\"headers\",host=\"a\",method=\"GET\",server=\"srv0\"} 12
caddy_http_request_duration_seconds_count{code=\"502\",handler=\"reverse_proxy\",host=\"a\",method=\"POST\",server=\"srv0\"} 3
caddy_http_request_duration_seconds_bucket{code=\"200\",handler=\"encode\",host=\"a\",method=\"GET\",server=\"srv0\",le=\"0.005\"} 3
other_metric{foo=\"bar\"} 7
";

	#[test]
	fn parses_caddy_metric_lines() {
		let counts = parse_status_counts(SAMPLE);
		assert_eq!(
			counts.into_iter().collect::<Vec<_>>(),
			vec![
				("200".to_string(), 9),
				("404".to_string(), 12),
				("502".to_string(), 3),
			]
		);
	}

	#[test]
	fn metrics_switch_read_from_the_app_or_its_servers() {
		use serde_json::json;

		// caddy 2.9 and later: the switch sits on the http app, and caddy
		// serialises it as an empty object when it carries no sub-options.
		assert!(metrics_enabled_in(&json!({ "metrics": {} })));
		assert!(metrics_enabled_in(
			&json!({ "metrics": { "per_host": true } })
		));

		// before 2.9: per server, and one instrumented server produces counters.
		assert!(metrics_enabled_in(&json!({
			"servers": { "srv0": { "metrics": {} }, "srv1": { "listen": [":80"] } }
		})));

		assert!(!metrics_enabled_in(&json!({
			"servers": { "srv0": { "listen": [":443"], "routes": [] } }
		})));
		assert!(!metrics_enabled_in(&json!({ "servers": {} })));
		assert!(!metrics_enabled_in(&json!({})));
		// caddy answers with a bare null when it has no http app at all
		assert!(!metrics_enabled_in(&Value::Null));
	}

	#[test]
	fn ignores_unrelated_metrics() {
		let counts = parse_status_counts("foo_bar{code=\"500\"} 99");
		assert!(counts.is_empty());
	}

	/// A certificate with only wildcard names has no host to dial, so nothing
	/// is served to compare it against. The check reads that as no comparison
	/// rather than as a mismatch.
	///
	/// spec: SUB#http-traffic-and-certificates
	#[test]
	fn a_wildcard_only_certificate_has_nothing_to_dial() {
		let wildcards = vec!["*.example.com".to_string(), "*.example.net".to_string()];
		assert!(name_to_dial(&wildcards).is_none());

		let mixed = vec!["*.example.com".to_string(), "host.example.com".to_string()];
		assert_eq!(
			name_to_dial(&mixed).map(String::as_str),
			Some("host.example.com")
		);
	}

	#[test]
	fn label_extract_simple() {
		assert_eq!(
			extract_label("{code=\"200\",server=\"srv0\"}", "code"),
			Some("200".to_string())
		);
	}

	fn disk_cert(sans: &[&str], not_after: i64, der: &[u8]) -> DiskCert {
		DiskCert {
			sans: sans.iter().map(|s| (*s).to_string()).collect(),
			not_before: not_after - 90 * D,
			not_after,
			der: der.to_vec(),
		}
	}

	fn origins(certs: &[(String, DiskCert)]) -> Vec<&str> {
		certs.iter().map(|(origin, _)| origin.as_str()).collect()
	}

	#[test]
	fn superseded_issuance_is_dropped() {
		// A host whose renewal moved from one CA to another keeps the old CA's
		// copy in the store, where it expires untouched. Only the live cert is
		// graded, so the leftover reports neither an expiry nor a mismatch.
		let live = disk_cert(&["app.example.com"], 100 * D, b"live");
		let superseded = disk_cert(&["app.example.com"], 10 * D, b"superseded");
		let kept = keep_live_certs(vec![
			("store/ca-b/app.example.com.crt".into(), superseded),
			("store/ca-a/app.example.com.crt".into(), live),
		]);
		assert_eq!(origins(&kept), vec!["store/ca-a/app.example.com.crt"]);
	}

	#[test]
	fn certs_for_different_names_are_all_kept() {
		let kept = keep_live_certs(vec![
			("a.crt".into(), disk_cert(&["a.example.com"], 100 * D, b"a")),
			("b.crt".into(), disk_cert(&["b.example.com"], 20 * D, b"b")),
		]);
		assert_eq!(origins(&kept).len(), 2);
	}

	#[test]
	fn multi_name_certs_group_regardless_of_san_order_or_case() {
		let live = disk_cert(&["b.example.com", "A.example.com"], 100 * D, b"live");
		let superseded = disk_cert(&["a.example.com", "b.example.com."], 10 * D, b"old");
		let kept = keep_live_certs(vec![
			("old.crt".into(), superseded),
			("live.crt".into(), live),
		]);
		assert_eq!(origins(&kept), vec!["live.crt"]);
	}

	#[test]
	fn certs_with_a_differing_name_set_are_not_superseded() {
		// Adding a name to a cert makes it a different cert, not a renewal of
		// the narrower one, and both stay in use.
		let both = disk_cert(&["a.example.com", "b.example.com"], 100 * D, b"both");
		let one = disk_cert(&["a.example.com"], 10 * D, b"one");
		let kept = keep_live_certs(vec![("one.crt".into(), one), ("both.crt".into(), both)]);
		assert_eq!(origins(&kept).len(), 2);
	}

	#[test]
	fn certs_without_names_are_kept() {
		let kept = keep_live_certs(vec![
			("x.crt".into(), disk_cert(&[], 100 * D, b"x")),
			("y.crt".into(), disk_cert(&[], 10 * D, b"y")),
		]);
		assert_eq!(origins(&kept).len(), 2);
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
