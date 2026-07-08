//! Seedling healthcheck.
//!
//! On a Seedling host, queries the local Seedling daemon over its OI control
//! interface and reports its subsystem health. Skips cleanly when the host
//! runs no Seedling, and reports broken (not failing) when the daemon cannot
//! be reached or has not authorised this client — a daemon-side condition, not
//! an unhealthy system.
//!
//! spec: SDH

use std::{
	net::{Ipv4Addr, SocketAddr},
	path::{Path, PathBuf},
};

use seedling_protocol::{
	actor::Actor,
	client::{ClientAuth, OiClient},
	keys::{self, ClientIdentity},
};
use serde_json::{Value, json};

use super::SweepContext;
use crate::doctor::check::Check;

const CHECK_NAME: &str = "seedling";

/// The Seedling daemon's OI listens here on loopback (mirrors the daemon's
/// `oi::DEFAULT_PORT`).
const OI_PORT: u16 = 7891;

pub async fn run(_ctx: SweepContext) -> Check {
	let Some(data_dir) = seedling_data_dir() else {
		return Check::skip(
			CHECK_NAME,
			"no Seedling on this host",
			"no Seedling data directory is configured in the environment",
		);
	};

	let server_key = data_dir.join("oi.key");
	if !server_key.exists() {
		return Check::skip(
			CHECK_NAME,
			"no Seedling on this host",
			format!("no Seedling OI key at {}", server_key.display()),
		);
	}

	match probe(&server_key).await {
		Ok(health) => health.into_check(),
		Err(reason) => Check::broken(CHECK_NAME, "could not query the Seedling daemon", reason),
	}
}

/// Where the Seedling daemon keeps its data. The daemon takes this as a launch
/// argument, so the doctor reads it from the same place the deployment sets it.
fn seedling_data_dir() -> Option<PathBuf> {
	std::env::var_os("SEEDLING_DATA_DIR").map(PathBuf::from)
}

/// This doctor's own client identity for the OI. Its fingerprint must be in the
/// daemon's authorised set for a query to be admitted.
fn client_key_path() -> PathBuf {
	dirs::state_dir()
		.or_else(dirs::data_local_dir)
		.unwrap_or_else(|| PathBuf::from("."))
		.join("bestool")
		.join("seedling-oi-client.key")
}

async fn probe(server_key_path: &Path) -> Result<Health, String> {
	// Pin the local daemon's fingerprint from its persisted key. The key is
	// present (checked by the caller), so this loads rather than generates.
	let server_key = keys::load_or_generate(server_key_path).map_err(|e| e.to_string())?;
	let server_fingerprint = keys::fingerprint(&keys::spki_der(&server_key));

	let (identity, _is_new) =
		ClientIdentity::load_or_generate(&client_key_path()).map_err(|e| e.to_string())?;

	let actor = Actor {
		kind: Some("bestool".to_owned()),
		id: Some("doctor".to_owned()),
		display: Some("bestool doctor".to_owned()),
		session: Some(identity.fingerprint[..8].to_owned()),
	};

	let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, OI_PORT));
	let client = OiClient::connect(
		addr,
		ClientAuth::Fingerprint(server_fingerprint),
		&identity,
		actor,
	)
	.await
	.map_err(|e| e.to_string())?;

	let infra = client
		.request("/infra/status", json!({}))
		.await
		.map_err(|e| e.to_string())?;
	let status = client
		.request("/server/status", json!({}))
		.await
		.map_err(|e| e.to_string())?;

	Ok(Health { infra, status })
}

struct Health {
	infra: Value,
	status: Value,
}

impl Health {
	fn into_check(self) -> Check {
		let str_field = |v: &Value, k: &str| {
			v.get(k)
				.and_then(Value::as_str)
				.unwrap_or("unknown")
				.to_owned()
		};
		let proxy = str_field(&self.infra, "proxy");
		let resolver = str_field(&self.infra, "resolver");
		let apps_total = self
			.status
			.get("apps_total")
			.and_then(Value::as_u64)
			.unwrap_or(0);

		let summary = format!("proxy {proxy}, resolver {resolver}, {apps_total} apps");

		let mut down = Vec::new();
		if proxy != "running" {
			down.push(format!("reverse proxy is {proxy}"));
		}
		if resolver != "running" {
			down.push(format!("DNS resolver is {resolver}"));
		}

		if down.is_empty() {
			Check::pass(CHECK_NAME, summary)
		} else {
			Check::fail(CHECK_NAME, summary, down.join("; "))
		}
	}
}
