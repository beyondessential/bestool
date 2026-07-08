//! Seedling healthchecks.
//!
//! On a Seedling host the doctor queries the local Seedling daemon over its OI
//! control interface once per sweep ([`probe`]) and derives one check per
//! subsystem: reverse proxy, DNS resolver, and managed apps. The checks skip
//! when the host runs no Seedling, and report broken (not failing) when the
//! daemon cannot be reached or has not authorised this client — a daemon-side
//! condition, not an unhealthy system.
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

/// The Seedling daemon's OI listens here on loopback (mirrors the daemon's
/// `oi::DEFAULT_PORT`).
const OI_PORT: u16 = 7891;

/// One sweep's view of the local Seedling daemon.
#[derive(Clone)]
pub struct SeedlingStatus {
	infra: Value,
	status: Value,
}

/// Resolved once per sweep and shared across the Seedling checks:
/// - `None` — no Seedling on this host (checks skip);
/// - `Some(Err(reason))` — Seedling is present but the daemon couldn't be
///   queried (checks report broken);
/// - `Some(Ok(status))` — the daemon answered.
pub type Probe = Option<Result<SeedlingStatus, String>>;

/// Query the local Seedling daemon, if this is a Seedling host.
pub async fn probe() -> Probe {
	let server_key = seedling_data_dir()?.join("oi.key");
	if !server_key.exists() {
		return None;
	}
	// The connect has its own 5s timeout but the requests don't; the outer
	// timeout keeps a wedged daemon from stalling the whole sweep.
	Some(
		match tokio::time::timeout(std::time::Duration::from_secs(10), query(&server_key)).await {
			Ok(result) => result,
			Err(_) => Err("timed out querying the Seedling daemon".into()),
		},
	)
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

async fn query(server_key_path: &Path) -> Result<SeedlingStatus, String> {
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

	Ok(SeedlingStatus { infra, status })
}

/// Turn the shared probe into a check, handling the skip/broken cases that are
/// identical across the three subsystems and delegating the healthy/unhealthy
/// call to `f`.
fn resolve(
	ctx: &SweepContext,
	name: &'static str,
	f: impl FnOnce(&SeedlingStatus) -> Check,
) -> Check {
	match &ctx.seedling {
		None => Check::skip(
			name,
			"no Seedling on this host",
			"no Seedling data directory (with an OI key) is configured on this host",
		),
		Some(Err(reason)) => {
			Check::broken(name, "could not query the Seedling daemon", reason.clone())
		}
		Some(Ok(status)) => f(status),
	}
}

fn str_field(v: &Value, key: &str) -> String {
	v.get(key)
		.and_then(Value::as_str)
		.unwrap_or("unknown")
		.to_owned()
}

pub async fn proxy(ctx: SweepContext) -> Check {
	resolve(&ctx, "seedling_proxy", |s| {
		let state = str_field(&s.infra, "proxy");
		if state == "running" {
			Check::pass("seedling_proxy", "reverse proxy running")
		} else {
			Check::fail(
				"seedling_proxy",
				format!("reverse proxy {state}"),
				"the Seedling reverse proxy is not running",
			)
		}
	})
}

pub async fn resolver(ctx: SweepContext) -> Check {
	resolve(&ctx, "seedling_resolver", |s| {
		let state = str_field(&s.infra, "resolver");
		if state == "running" {
			Check::pass("seedling_resolver", "DNS resolver running")
		} else {
			Check::fail(
				"seedling_resolver",
				format!("DNS resolver {state}"),
				"the Seedling DNS resolver is not running",
			)
		}
	})
}

pub async fn apps(ctx: SweepContext) -> Check {
	resolve(&ctx, "seedling_apps", |s| {
		let total = s
			.status
			.get("apps_total")
			.and_then(Value::as_u64)
			.unwrap_or(0);
		let running = s
			.status
			.get("apps_by_status")
			.and_then(Value::as_object)
			.and_then(|m| m.get("running"))
			.and_then(Value::as_u64)
			.unwrap_or(0);

		if total == 0 {
			Check::pass("seedling_apps", "no apps deployed")
		} else if running >= total {
			Check::pass("seedling_apps", format!("{total} apps running"))
		} else {
			Check::warning(
				"seedling_apps",
				format!("{running}/{total} apps running"),
				format!("{} app(s) not running", total - running),
			)
		}
	})
}
