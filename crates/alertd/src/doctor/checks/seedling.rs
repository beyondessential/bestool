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

use ed25519_dalek::{SigningKey, pkcs8::DecodePrivateKey};
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
	let data_dir = seedling_data_dir()?;
	let fingerprint = match server_fingerprint(&data_dir)? {
		Ok(fp) => fp,
		Err(reason) => return Some(Err(reason)),
	};
	// The connect has its own 5s timeout but the requests don't; the outer
	// timeout keeps a wedged daemon from stalling the whole sweep.
	Some(
		match tokio::time::timeout(std::time::Duration::from_secs(10), query(fingerprint)).await {
			Ok(result) => result,
			Err(_) => Err("timed out querying the Seedling daemon".into()),
		},
	)
}

/// The daemon's OI fingerprint to pin, from the public `oi.fingerprint` file
/// the daemon publishes beside its key; for daemons that predate the file,
/// derived from the key itself. `None` when neither exists — no Seedling here.
fn server_fingerprint(data_dir: &Path) -> Option<Result<String, String>> {
	let fingerprint_path = data_dir.join("oi.fingerprint");
	if fingerprint_path.exists() {
		return Some(
			std::fs::read_to_string(&fingerprint_path)
				.map(|s| s.trim().to_owned())
				.map_err(|e| e.to_string()),
		);
	}
	let key_path = data_dir.join("oi.key");
	if !key_path.exists() {
		return None;
	}
	// Strictly read-only: never `keys::load_or_generate` here, which would
	// create a fresh key at the daemon's path if the file vanished between
	// the check above and the load — silently replacing the daemon's identity.
	Some(
		std::fs::read(&key_path)
			.map_err(|e| e.to_string())
			.and_then(|der| SigningKey::from_pkcs8_der(&der).map_err(|e| e.to_string()))
			.map(|key| keys::fingerprint(&keys::spki_der(&key))),
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

async fn query(server_fingerprint: String) -> Result<SeedlingStatus, String> {
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

/// A subsystem's state from `/infra/status`. A response missing the field is a
/// daemon that cannot answer this check, which per SDH is broken, not failing.
fn infra_check(s: &SeedlingStatus, name: &'static str, key: &str, label: &str) -> Check {
	match s.infra.get(key).and_then(Value::as_str) {
		None => Check::broken(
			name,
			format!("daemon did not report the {label}"),
			format!("`/infra/status` response has no string `{key}` field"),
		),
		Some("running") => Check::pass(name, format!("{label} running")),
		Some(state) => Check::fail(
			name,
			format!("{label} {state}"),
			format!("the Seedling {label} is not running"),
		),
	}
}

pub async fn proxy(ctx: SweepContext) -> Check {
	resolve(&ctx, "seedling_proxy", |s| {
		infra_check(s, "seedling_proxy", "proxy", "reverse proxy")
	})
}

pub async fn resolver(ctx: SweepContext) -> Check {
	resolve(&ctx, "seedling_resolver", |s| {
		infra_check(s, "seedling_resolver", "resolver", "DNS resolver")
	})
}

pub async fn apps(ctx: SweepContext) -> Check {
	resolve(&ctx, "seedling_apps", |s| {
		let (Some(total), Some(by_status)) = (
			s.status.get("apps_total").and_then(Value::as_u64),
			s.status.get("apps_by_status").and_then(Value::as_object),
		) else {
			// Missing counts are a daemon that cannot answer: broken, per SDH.
			return Check::broken(
				"seedling_apps",
				"daemon did not report app counts",
				"`/server/status` response is missing `apps_total` or `apps_by_status`",
			);
		};
		// The daemon only lists statuses that occur, so no `running` entry
		// means zero apps running, not a malformed response.
		let running = by_status
			.get("running")
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
