//! Whether the machine, or the Tamanu on it, trusts a distrusted root CA.
//!
//! spec: CHK-DTR

use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::MachineCx;
use crate::check::Check;

const NAME: &str = "distrusted_roots";

/// A root certificate authority no trust store should carry.
#[derive(Debug, PartialEq, Eq)]
pub struct DistrustedRoot {
	pub name: &'static str,
	/// SHA-256 of the certificate's DER, upper-case hex with colons, the form
	/// Node.js prints.
	pub fingerprint: &'static str,
}

// spec: CHK-DTR#distrusted-roots
pub const DISTRUSTED_ROOTS: &[DistrustedRoot] = &[DistrustedRoot {
	name: "emSign ECC Root CA - G3",
	fingerprint: "86:A1:EC:BA:08:9C:4A:8D:3B:BE:27:34:C6:12:BA:34:1D:81:3E:04:3C:F9:E8:A8:62:CD:5C:57:A3:6B:BE:6B",
}];

/// Prints, as JSON, the fingerprints of the Node.js default trust store and of
/// the system bundle beside it, or why the bundle could not be read. A Node.js
/// too old to report its default store reports its built-in roots instead,
/// which leaves out any added with `NODE_EXTRA_CA_CERTS`.
const NODE_SCRIPT: &str = r#"
const tls = require('tls');
const { X509Certificate } = require('crypto');
const fs = require('fs');
const fingerprint = pem => new X509Certificate(pem).fingerprint256;
const node = (tls.getCACertificates ? tls.getCACertificates('default') : tls.rootCertificates).map(fingerprint);
let system = null;
let systemError = null;
try {
	system = (fs.readFileSync('/etc/ssl/certs/ca-certificates.crt', 'utf8')
		.match(/-----BEGIN CERTIFICATE-----[\s\S]+?-----END CERTIFICATE-----/g) ?? []).map(fingerprint);
} catch (err) {
	systemError = String(err);
}
console.log(JSON.stringify({ node, system, systemError }));
"#;

const CONTAINER_NODE_STORE: &str = "Tamanu container Node.js store";
const CONTAINER_SYSTEM_BUNDLE: &str = "Tamanu container system bundle";

/// What one trust store said about the distrusted roots.
#[derive(Debug)]
pub struct Reading {
	pub store: &'static str,
	pub outcome: Result<Vec<&'static DistrustedRoot>, String>,
}

// spec: CHK-DTR#trust-stores
pub async fn run(ctx: MachineCx) -> Check {
	let mut readings = vec![Reading {
		store: "host trust store",
		outcome: host_trusted(),
	}];

	if let Some(tamanu) = &ctx.tamanu {
		readings.extend(tamanu_readings(tamanu.root.as_deref()).await);
	}

	grade(&readings)
}

#[cfg(target_os = "linux")]
async fn tamanu_readings(_root: Option<&std::path::Path>) -> Vec<Reading> {
	match bestool_tamanu::versions::container_node_eval(NODE_SCRIPT).await {
		Ok(None) => Vec::new(),
		Ok(Some(output)) => container_readings(parse_node_output(&output)),
		Err(err) => container_readings(Err(err)),
	}
}

/// The container's two stores, both unreadable when the listing is.
#[cfg_attr(
	not(target_os = "linux"),
	expect(dead_code, reason = "containers are only read on Linux")
)]
fn container_readings(stores: Result<NodeStores, String>) -> Vec<Reading> {
	match stores {
		Ok(stores) => vec![
			Reading {
				store: CONTAINER_NODE_STORE,
				outcome: Ok(trusted_among(&stores.node)),
			},
			Reading {
				store: CONTAINER_SYSTEM_BUNDLE,
				outcome: match (stores.system, stores.system_error) {
					(Some(system), _) => Ok(trusted_among(&system)),
					(None, error) => Err(error.unwrap_or_else(|| "not reported".into())),
				},
			},
		],
		Err(err) => vec![
			Reading {
				store: CONTAINER_NODE_STORE,
				outcome: Err(err.clone()),
			},
			Reading {
				store: CONTAINER_SYSTEM_BUNDLE,
				outcome: Err(err),
			},
		],
	}
}

#[cfg(windows)]
async fn tamanu_readings(root: Option<&std::path::Path>) -> Vec<Reading> {
	let Some(root) = root else {
		return Vec::new();
	};
	let store = "Tamanu bundled Node.js store";
	match bestool_tamanu::server_info::embedded_node_eval(root, NODE_SCRIPT).await {
		Ok(None) => Vec::new(),
		Ok(Some(output)) => vec![Reading {
			store,
			outcome: parse_node_output(&output).map(|stores| trusted_among(&stores.node)),
		}],
		Err(err) => vec![Reading {
			store,
			outcome: Err(err),
		}],
	}
}

#[cfg(not(any(target_os = "linux", windows)))]
async fn tamanu_readings(_root: Option<&std::path::Path>) -> Vec<Reading> {
	Vec::new()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeStores {
	node: Vec<String>,
	system: Option<Vec<String>>,
	system_error: Option<String>,
}

fn parse_node_output(output: &str) -> Result<NodeStores, String> {
	serde_json::from_str(output.trim())
		.map_err(|err| format!("unreadable trust store listing: {err}"))
}

/// Windows fetches roots from its root program as they are needed, so absence
/// from the local root store says nothing; only the untrusted store does.
#[cfg(windows)]
fn host_trusted() -> Result<Vec<&'static DistrustedRoot>, String> {
	let store = schannel::cert_store::CertStore::open_local_machine("Disallowed")
		.map_err(|err| format!("could not open the untrusted certificates store: {err}"))?;
	let untrusted: Vec<String> = store
		.certs()
		.map(|cert| fingerprint_of(cert.to_der()))
		.collect();
	Ok(DISTRUSTED_ROOTS
		.iter()
		.filter(|root| !untrusted.iter().any(|fp| fp == root.fingerprint))
		.collect())
}

#[cfg(not(windows))]
fn host_trusted() -> Result<Vec<&'static DistrustedRoot>, String> {
	let result = rustls_native_certs::load_native_certs();
	let fingerprints: Vec<String> = result
		.certs
		.iter()
		.map(|cert| fingerprint_of(cert))
		.collect();
	let errors: Vec<String> = result.errors.iter().map(ToString::to_string).collect();
	host_store_outcome(&fingerprints, &errors)
}

/// A distrusted root found is trusted whatever else failed to load; otherwise a
/// store that loaded with errors, or loaded nothing at all, was not fully read.
#[cfg_attr(
	windows,
	expect(dead_code, reason = "Windows reads its untrusted store")
)]
fn host_store_outcome(
	fingerprints: &[String],
	errors: &[String],
) -> Result<Vec<&'static DistrustedRoot>, String> {
	let trusted = trusted_among(fingerprints);
	if !trusted.is_empty() {
		return Ok(trusted);
	}
	if !errors.is_empty() {
		return Err(format!(
			"could not fully load the host trust store: {}",
			errors.join("; ")
		));
	}
	if fingerprints.is_empty() {
		return Err("the host trust store has no certificates".into());
	}
	Ok(trusted)
}

fn fingerprint_of(der: &[u8]) -> String {
	Sha256::digest(der)
		.iter()
		.map(|byte| format!("{byte:02X}"))
		.collect::<Vec<_>>()
		.join(":")
}

fn trusted_among(fingerprints: &[String]) -> Vec<&'static DistrustedRoot> {
	DISTRUSTED_ROOTS
		.iter()
		.filter(|root| {
			fingerprints
				.iter()
				.any(|fp| fp.eq_ignore_ascii_case(root.fingerprint))
		})
		.collect()
}

// spec: CHK-DTR#outcome
fn grade(readings: &[Reading]) -> Check {
	let mut details = Map::new();
	for reading in readings {
		let value = match &reading.outcome {
			Ok(roots) => {
				json!({ "trusts": roots.iter().map(|root| root.name).collect::<Vec<_>>() })
			}
			Err(err) => json!({ "unreadable": err }),
		};
		details.insert(reading.store.to_string(), value);
	}
	let details = Map::from_iter([("stores".to_string(), Value::Object(details))]);

	let trusted: Vec<String> = DISTRUSTED_ROOTS
		.iter()
		.filter_map(|root| {
			let stores: Vec<&str> = readings
				.iter()
				.filter(|reading| {
					reading
						.outcome
						.as_ref()
						.is_ok_and(|roots| roots.contains(&root))
				})
				.map(|reading| reading.store)
				.collect();
			(!stores.is_empty()).then(|| format!("{} trusted by {}", root.name, stores.join(", ")))
		})
		.collect();
	if !trusted.is_empty() {
		return Check::fail(NAME, "distrusted root CA trusted", trusted.join("; "))
			.with_details(details);
	}

	let unreadable: Vec<&str> = readings
		.iter()
		.filter(|reading| reading.outcome.is_err())
		.map(|reading| reading.store)
		.collect();
	if !unreadable.is_empty() {
		return Check::warning(
			NAME,
			"some trust stores unreadable",
			format!("could not read {}", unreadable.join(", ")),
		)
		.with_details(details);
	}

	Check::pass(NAME, "no distrusted root CA trusted").with_details(details)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::check::CheckStatus;

	const G3: &DistrustedRoot = &DISTRUSTED_ROOTS[0];

	#[test]
	fn fingerprints_match_node_format() {
		assert_eq!(
			fingerprint_of(b"abc"),
			"BA:78:16:BF:8F:01:CF:EA:41:41:40:DE:5D:AE:22:23:B0:03:61:A3:96:17:7A:9C:B4:10:FF:61:F2:00:15:AD"
		);
	}

	#[test]
	fn finds_distrusted_root_among_fingerprints() {
		let fingerprints = vec!["00:11".to_string(), G3.fingerprint.to_ascii_lowercase()];
		assert_eq!(trusted_among(&fingerprints), vec![G3]);
		assert!(trusted_among(&["00:11".to_string()]).is_empty());
	}

	/// emSign ECC Root CA - G3, as Node.js ships it.
	const G3_PEM: &str = "-----BEGIN CERTIFICATE-----
MIICTjCCAdOgAwIBAgIKPPYHqWhwDtqLhDAKBggqhkjOPQQDAzBrMQswCQYDVQQGEwJJTjET
MBEGA1UECxMKZW1TaWduIFBLSTElMCMGA1UEChMcZU11ZGhyYSBUZWNobm9sb2dpZXMgTGlt
aXRlZDEgMB4GA1UEAxMXZW1TaWduIEVDQyBSb290IENBIC0gRzMwHhcNMTgwMjE4MTgzMDAw
WhcNNDMwMjE4MTgzMDAwWjBrMQswCQYDVQQGEwJJTjETMBEGA1UECxMKZW1TaWduIFBLSTEl
MCMGA1UEChMcZU11ZGhyYSBUZWNobm9sb2dpZXMgTGltaXRlZDEgMB4GA1UEAxMXZW1TaWdu
IEVDQyBSb290IENBIC0gRzMwdjAQBgcqhkjOPQIBBgUrgQQAIgNiAAQjpQy4LRL1KPOxst3i
AhKAnjlfSU2fySU0WXTsuwYc58Byr+iuL+FBVIcUqEqy6HyC5ltqtdyzdc6LBtCGI79G1Y4P
PwT01xySfvalY8L1X44uT6EYGQIrMgqCZH0Wk9GjQjBAMB0GA1UdDgQWBBR8XQKEE9TMipuB
zhccLikenEhjQjAOBgNVHQ8BAf8EBAMCAQYwDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQD
AwNpADBmAjEAvvNhzwIQHWSVB7gYboiFBS+DCBeQyh+KTOgNG3qxrdWBCUfvO6wIBHxcmbHt
RwfSAjEAnbpV/KlK6O3t5nYBQnvI+GDZjVGLVTv7jHvrZQnD+JbNR6iC8hZVdyR+EhCVBCyj
-----END CERTIFICATE-----
";

	#[test]
	fn fingerprint_is_that_of_the_certificate() {
		let (_, pem) = x509_parser::pem::parse_x509_pem(G3_PEM.as_bytes()).unwrap();
		let cert = pem.parse_x509().unwrap();
		assert_eq!(
			cert.subject().to_string(),
			"C=IN, OU=emSign PKI, O=eMudhra Technologies Limited, CN=emSign ECC Root CA - G3"
		);
		assert_eq!(fingerprint_of(&pem.contents), G3.fingerprint);
	}

	#[test]
	fn parses_node_output() {
		let stores = parse_node_output(&format!(
			"{{\"node\":[\"{}\"],\"system\":null,\"systemError\":\"ENOENT\"}}\n",
			G3.fingerprint
		))
		.unwrap();
		assert_eq!(trusted_among(&stores.node), vec![G3]);
		assert!(stores.system.is_none());
		assert_eq!(stores.system_error.as_deref(), Some("ENOENT"));
		assert!(parse_node_output("not json").is_err());
	}

	#[test]
	fn an_unreadable_container_bundle_is_unreadable_not_absent() {
		let readings = container_readings(Ok(NodeStores {
			node: vec![],
			system: None,
			system_error: Some("EACCES".into()),
		}));
		assert_eq!(readings.len(), 2);
		assert_eq!(readings[1].store, CONTAINER_SYSTEM_BUNDLE);
		assert!(matches!(&readings[1].outcome, Err(err) if err == "EACCES"));
	}

	#[test]
	fn an_unreadable_listing_leaves_both_container_stores_unread() {
		let readings = container_readings(Err("podman ps exited with 125".into()));
		assert_eq!(readings.len(), 2);
		assert!(readings.iter().all(|reading| reading.outcome.is_err()));
	}

	#[test]
	fn host_store_load_errors_or_emptiness_are_not_a_clean_read() {
		let other = vec!["00:11".to_string()];
		let with_g3 = vec!["00:11".to_string(), G3.fingerprint.to_string()];
		let errors = vec!["unreadable file".to_string()];

		assert_eq!(host_store_outcome(&with_g3, &errors).unwrap(), vec![G3]);
		assert!(host_store_outcome(&other, &errors).is_err());
		assert!(host_store_outcome(&[], &[]).is_err());
		assert!(host_store_outcome(&other, &[]).unwrap().is_empty());
	}

	#[test]
	fn fails_naming_each_store_that_trusts() {
		let check = grade(&[
			Reading {
				store: "host trust store",
				outcome: Ok(vec![]),
			},
			Reading {
				store: "Tamanu container Node.js store",
				outcome: Ok(vec![G3]),
			},
			Reading {
				store: "Tamanu container system bundle",
				outcome: Err("boom".into()),
			},
		]);
		assert!(matches!(
			check.status,
			CheckStatus::Fail(reason)
				if reason == "emSign ECC Root CA - G3 trusted by Tamanu container Node.js store"
		));
	}

	#[test]
	fn warns_when_a_store_is_unreadable() {
		let check = grade(&[
			Reading {
				store: "host trust store",
				outcome: Ok(vec![]),
			},
			Reading {
				store: "Tamanu bundled Node.js store",
				outcome: Err("boom".into()),
			},
		]);
		assert!(matches!(
			check.status,
			CheckStatus::Warning(reason) if reason == "could not read Tamanu bundled Node.js store"
		));
	}

	#[test]
	fn passes_when_nothing_trusts() {
		let check = grade(&[Reading {
			store: "host trust store",
			outcome: Ok(vec![]),
		}]);
		assert!(matches!(check.status, CheckStatus::Pass));
	}
}
