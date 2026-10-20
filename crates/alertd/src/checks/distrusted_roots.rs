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

/// Prints the fingerprints of the Node.js default trust store and of the system
/// bundle beside it, as JSON. Falls back to the built-in roots on a Node.js too
/// old to report its default store.
const NODE_SCRIPT: &str = r#"
const tls = require('tls');
const { X509Certificate } = require('crypto');
const fs = require('fs');
const fingerprint = pem => new X509Certificate(pem).fingerprint256;
const node = (tls.getCACertificates ? tls.getCACertificates('default') : tls.rootCertificates).map(fingerprint);
let system = null;
try {
	system = (fs.readFileSync('/etc/ssl/certs/ca-certificates.crt', 'utf8')
		.match(/-----BEGIN CERTIFICATE-----[\s\S]+?-----END CERTIFICATE-----/g) ?? []).map(fingerprint);
} catch {}
console.log(JSON.stringify({ node, system }));
"#;

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
		Ok(Some(output)) => match parse_node_output(&output) {
			Ok(stores) => {
				let mut readings = vec![Reading {
					store: "Tamanu container Node.js store",
					outcome: Ok(trusted_among(&stores.node)),
				}];
				if let Some(system) = stores.system {
					readings.push(Reading {
						store: "Tamanu container system bundle",
						outcome: Ok(trusted_among(&system)),
					});
				}
				readings
			}
			Err(err) => vec![Reading {
				store: "Tamanu container Node.js store",
				outcome: Err(err),
			}],
		},
		Err(err) => vec![Reading {
			store: "Tamanu container Node.js store",
			outcome: Err(err),
		}],
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
struct NodeStores {
	node: Vec<String>,
	#[cfg_attr(
		windows,
		expect(dead_code, reason = "a bundled runtime has no system bundle beside it")
	)]
	system: Option<Vec<String>>,
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
	if result.certs.is_empty()
		&& let Some(err) = result.errors.first()
	{
		return Err(format!("could not load the host trust store: {err}"));
	}
	let fingerprints: Vec<String> = result
		.certs
		.iter()
		.map(|cert| fingerprint_of(cert))
		.collect();
	Ok(trusted_among(&fingerprints))
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

	#[test]
	fn parses_node_output() {
		let stores = parse_node_output(&format!(
			"{{\"node\":[\"{}\"],\"system\":null}}\n",
			G3.fingerprint
		))
		.unwrap();
		assert_eq!(trusted_among(&stores.node), vec![G3]);
		assert!(stores.system.is_none());
		assert!(parse_node_output("not json").is_err());
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
