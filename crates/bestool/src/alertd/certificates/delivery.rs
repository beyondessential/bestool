//! The certificate endpoint Caddy asks during a handshake.
//!
//! Caddy's `get_certificate http` manager issues an ordinary GET carrying the
//! name from the client's server name indication, the signature schemes and
//! cipher suites the client offered, and the local address it connected to. A
//! `200` carrying the chain and its key as PEM is served; a `204` declines and
//! hands the handshake back to Caddy, which issues for the name itself.
//!
//! Anything else ends the handshake: Caddy reads an error as the manager having
//! been unable to serve a certificate it was responsible for, and does not fall
//! back to its own issuance. So the endpoint declines wherever it can, and fails
//! only when it cannot answer at all — which is the refused caller, deliberately.
//!
//! Every answer comes from state the daemon already holds in memory. The
//! handshake is blocked until it answers and Caddy applies no timeout to the
//! request, so the handler reaches no network, opens no key store and reads no
//! file.
//!
//! spec: TLSD

use std::{collections::BTreeSet, net::SocketAddr, sync::Arc, time::Duration};

use axum::{
	body::Body,
	extract::{ConnectInfo, Query, State},
	http::{HeaderValue, StatusCode, header::CONTENT_TYPE},
	response::{IntoResponse, Response},
};
use miette::{IntoDiagnostic as _, Result, WrapErr as _};
use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, warn};

use super::peer;
use crate::alertd::http_server::ServerState;

/// What Caddy's certificate manager passes.
///
/// Only the server name is acted on. The signature schemes and cipher suites
/// beside it are what the client offered, which a manager choosing between
/// several chains for one name would use; there is one chain per name here, so
/// they go unread rather than being made into a reason to refuse a well-formed
/// request.
#[derive(Debug, Deserialize)]
pub struct CertificateQuery {
	#[serde(default)]
	pub server_name: String,
}

/// Both ends of an accepted connection, which together name the caller's socket
/// in the kernel's table.
///
/// Axum's own connect info carries only the far end; the near end comes off the
/// accepted stream.
#[derive(Debug, Clone, Copy)]
pub struct Endpoints {
	pub remote: SocketAddr,
	pub local: SocketAddr,
}

impl
	axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, tokio::net::TcpListener>>
	for Endpoints
{
	fn connect_info(stream: axum::serve::IncomingStream<'_, tokio::net::TcpListener>) -> Self {
		let remote = *stream.remote_addr();
		// A connected socket always knows its own address; where it somehow
		// does not, this names a pair no entry in the kernel's table can match,
		// so the caller is refused rather than waved through.
		let local = stream.io().local_addr().unwrap_or(remote);
		Self { remote, local }
	}
}

/// Route handler for `/certificate`.
pub async fn handle_certificate(
	State(state): State<Arc<ServerState>>,
	ConnectInfo(ends): ConnectInfo<Endpoints>,
	Query(query): Query<CertificateQuery>,
) -> Response {
	let Some(certificates) = state.certificates.as_ref() else {
		// Nothing is collecting on this host, so there is no name this endpoint
		// is responsible for: decline, and let Caddy issue as it always has.
		return StatusCode::NO_CONTENT.into_response();
	};

	// The endpoint hands out a private key, so it identifies its caller rather
	// than accepting any connection that reaches it. A caller that is not
	// permitted is refused, and that refusal is a failure rather than a decline:
	// it is a misconfiguration to correct, not a name for Caddy to begin issuing
	// for itself.
	if let Err(err) = peer::caller_permitted(certificates.permitted(), ends.remote, ends.local) {
		warn!(peer = %ends.remote, %err, "refused a certificate request");
		return (StatusCode::FORBIDDEN, format!("{err}")).into_response();
	}

	if query.server_name.is_empty() {
		return (
			StatusCode::BAD_REQUEST,
			"a certificate request must carry a server_name",
		)
			.into_response();
	}

	match certificates.serve(&query.server_name).await {
		Some((chain, key)) => {
			debug!(name = %query.server_name, "serving a canopy-issued chain");
			let mut body = chain;
			if !body.ends_with('\n') {
				body.push('\n');
			}
			body.push_str(&key);
			let mut response = Response::new(Body::from(body));
			response.headers_mut().insert(
				CONTENT_TYPE,
				HeaderValue::from_static("application/x-pem-file"),
			);
			response
		}
		None => {
			// Record the name so an order follows, then decline. Caddy asks only
			// for names it is configured to serve, so this is immediacy between
			// passes rather than a route of its own.
			certificates.note_wanted(&query.server_name).await;
			StatusCode::NO_CONTENT.into_response()
		}
	}
}

/// The admin API a local Caddy answers on.
const CONFIG_URL: &str = "http://localhost:2019/config/";
const CONFIG_TIMEOUT: Duration = Duration::from_secs(3);

/// The site addresses Caddy is configured to serve, read from its live admin
/// configuration.
///
/// This is where the names to certify come from: the daemon orders for them
/// ahead of any client arriving, because a certificate is obtained before it is
/// needed rather than while a client waits.
///
/// spec: TLS#which-names-are-certified
pub async fn caddy_subjects(client: &reqwest::Client) -> Result<BTreeSet<String>> {
	let response = client
		.get(CONFIG_URL)
		.timeout(CONFIG_TIMEOUT)
		.send()
		.await
		.into_diagnostic()
		.wrap_err("reading the caddy admin API at localhost:2019")?;
	if !response.status().is_success() {
		return Err(miette::miette!(
			"the caddy admin API answered {}",
			response.status()
		));
	}
	let config: Value = response
		.json()
		.await
		.into_diagnostic()
		.wrap_err("parsing caddy's configuration")?;
	Ok(active_subjects(&config))
}

/// Every hostname Caddy considers active, gathered from its configuration: route
/// `host` matchers (anywhere, including subroutes), plus TLS `automate` and
/// automation-policy `subjects`.
///
/// The same reading `caddy_certs` takes, for the same reason: the live
/// configuration is what says which names the host answers on.
fn active_subjects(config: &Value) -> BTreeSet<String> {
	fn walk(value: &Value, out: &mut BTreeSet<String>) {
		match value {
			Value::Object(map) => {
				for (key, val) in map {
					if matches!(key.as_str(), "host" | "automate" | "subjects")
						&& let Some(arr) = val.as_array()
					{
						out.extend(
							arr.iter()
								.filter_map(|v| v.as_str())
								.map(|s| s.trim_end_matches('.').to_ascii_lowercase()),
						);
					}
					walk(val, out);
				}
			}
			Value::Array(arr) => arr.iter().for_each(|v| walk(v, out)),
			_ => {}
		}
	}
	let mut out = BTreeSet::new();
	walk(config, &mut out);
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_names_to_certify_come_from_caddys_live_configuration() {
		let config = serde_json::json!({
			"apps": {
				"http": { "servers": { "srv0": { "routes": [
					{ "match": [{ "host": ["a.example.com", "B.example.com."] }],
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
	fn a_configuration_naming_no_site_names_nothing() {
		assert!(active_subjects(&serde_json::json!({"apps": {}})).is_empty());
	}
}
