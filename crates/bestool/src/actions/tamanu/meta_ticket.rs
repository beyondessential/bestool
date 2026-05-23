use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use clap::Parser;
use miette::{IntoDiagnostic as _, Result, miette};
use p256::pkcs8::DecodePrivateKey as _;
use serde::Serialize;
use sysinfo::System;
use tracing::debug;

use crate::actions::{
	Context,
	tamanu::{
		TamanuArgs,
		config::load_config,
		connection_url::ConnectionUrlBuilder,
		find_tamanu,
		server_info::{
			detect_virtualisation, get_or_create_device_key, get_or_create_server_id,
			get_tailscale_info,
		},
	},
};

/// Generate a meta-ticket for this Tamanu server
///
/// Connects to the Tamanu database, retrieves the device key, and produces
/// a base64-encoded JSON ticket containing server identity information.
#[derive(Debug, Clone, Parser)]
pub struct MetaTicketArgs;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Ticket {
	v: &'static str,
	kind: &'static str,
	server_id: String,
	public_key: String,
	hostname: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	tailscale_ip: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	tailscale_name: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	canonical_url: Option<String>,
	#[serde(skip_serializing_if = "Option::is_none")]
	hosting: Option<String>,
}

pub async fn run(_args: MetaTicketArgs, ctx: Context) -> Result<()> {
	let (_, root) = find_tamanu(ctx.require::<TamanuArgs>())?;
	let config = load_config(&root, None)?;

	let builder = ConnectionUrlBuilder {
		username: config.db.username.clone(),
		password: Some(config.db.password.clone()),
		host: config
			.db
			.host
			.clone()
			.unwrap_or_else(|| "localhost".to_string()),
		port: config.db.port,
		database: config.db.name.clone(),
		ssl_mode: None,
	};
	let url = builder.build();

	debug!(url, "connecting to database");
	let pool = bestool_psql::create_pool(&url).await?;
	let client = pool.get().await.into_diagnostic()?;

	let device_key_pem = get_or_create_device_key(&client).await?;

	let public_key_pem = derive_public_key_pem(&device_key_pem)?;

	let server_id = get_or_create_server_id(&client).await?;

	let (tailscale_ip, tailscale_name) = get_tailscale_info();
	let hosting = detect_hosting();

	let kind = if config.is_facility() {
		"facility"
	} else {
		"central"
	};

	let ticket = Ticket {
		v: "ticket-1",
		kind,
		server_id,
		public_key: public_key_pem,
		hostname: System::host_name(),
		tailscale_ip,
		tailscale_name,
		canonical_url: config.canonical_url().map(|u| u.to_string()),
		hosting,
	};

	let json = serde_json::to_string(&ticket).into_diagnostic()?;
	debug!(json, "ticket payload");
	let encoded = URL_SAFE_NO_PAD.encode(json.as_bytes());
	println!("{encoded}");

	Ok(())
}

fn derive_public_key_pem(private_key_pem: &str) -> Result<String> {
	use p256::elliptic_curve::pkcs8::EncodePublicKey as _;

	let secret_key = p256::SecretKey::from_pkcs8_pem(private_key_pem)
		.map_err(|e| miette!("failed to parse device key PEM: {e}"))?;
	let public_key = secret_key.public_key();
	let pem = public_key
		.to_public_key_pem(p256::pkcs8::LineEnding::LF)
		.map_err(|e| miette!("failed to encode public key PEM: {e}"))?;
	Ok(pem)
}

fn detect_hosting() -> Option<String> {
	if is_raspberry_pi() {
		return Some("iti".to_string());
	}

	if is_ec2() {
		return Some("ec2".to_string());
	}

	match detect_virtualisation() {
		Some(virt) if virt == "none" => Some("bare metal".to_string()),
		Some(virt) => Some(virt),
		None => None,
	}
}

fn is_raspberry_pi() -> bool {
	if let Ok(model) = std::fs::read_to_string("/proc/device-tree/model")
		&& model.to_lowercase().contains("raspberry pi")
	{
		return true;
	}

	false
}

fn is_ec2() -> bool {
	if let Ok(vendor) = std::fs::read_to_string("/sys/class/dmi/id/board_vendor")
		&& vendor.trim() == "Amazon EC2"
	{
		return true;
	}

	if let Ok(bios) = std::fs::read_to_string("/sys/class/dmi/id/bios_vendor")
		&& bios.trim().contains("Amazon")
	{
		return true;
	}

	if let Ok(hypervisor) = std::fs::read_to_string("/sys/hypervisor/uuid")
		&& hypervisor.trim().starts_with("ec2")
	{
		return true;
	}

	if std::fs::metadata("/sys/devices/virtual/dmi/id/board_asset_tag")
		.map(|m| m.is_file())
		.unwrap_or(false)
		&& let Ok(tag) = std::fs::read_to_string("/sys/devices/virtual/dmi/id/board_asset_tag")
		&& tag.trim().starts_with("i-")
	{
		return true;
	}

	false
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_derive_public_key_pem() {
		let private_pem = "\
-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgVvhzsYiidp38GYn1
KxD5Wipc/h8lglVsy1UFZq/SZbGhRANCAAT2EsEq7xjeWVnim9XwdYXga/LBbppm
fXLgamTYOa/w9n/Ta64fiYWmN54kEd0DgnflJDLtID321Zz6xswvK/VN
-----END PRIVATE KEY-----";

		let public_pem = derive_public_key_pem(private_pem).unwrap();
		assert!(public_pem.starts_with("-----BEGIN PUBLIC KEY-----"));
		assert!(public_pem.ends_with("-----END PUBLIC KEY-----\n"));
	}

	#[test]
	fn test_ticket_serialization() {
		let ticket = Ticket {
			v: "ticket-1",
			kind: "central",
			server_id: "abc-123".to_string(),
			public_key: "test-key".to_string(),
			hostname: Some("test-host".to_string()),
			tailscale_ip: None,
			tailscale_name: None,
			canonical_url: Some("https://example.com".to_string()),
			hosting: Some("ec2".to_string()),
		};

		let json = serde_json::to_string(&ticket).unwrap();
		let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
		assert_eq!(parsed["v"], "ticket-1");
		assert_eq!(parsed["kind"], "central");
		assert_eq!(parsed["serverId"], "abc-123");
		assert_eq!(parsed["publicKey"], "test-key");
		assert_eq!(parsed["hostname"], "test-host");
		assert!(parsed.get("tailscaleIp").is_none());
		assert!(parsed.get("tailscaleName").is_none());
		assert_eq!(parsed["canonicalUrl"], "https://example.com");
		assert_eq!(parsed["hosting"], "ec2");
	}

	#[test]
	fn test_ticket_base64_roundtrip() {
		let ticket = Ticket {
			v: "ticket-1",
			kind: "facility",
			server_id: "id-1".to_string(),
			public_key: "pk".to_string(),
			hostname: Some("h".to_string()),
			tailscale_ip: Some("100.1.2.3".to_string()),
			tailscale_name: Some("myhost.tail1234.ts.net".to_string()),
			canonical_url: None,
			hosting: None,
		};

		let json = serde_json::to_string(&ticket).unwrap();
		let encoded = URL_SAFE_NO_PAD.encode(json.as_bytes());
		let decoded = URL_SAFE_NO_PAD.decode(&encoded).unwrap();
		let decoded_json = String::from_utf8(decoded).unwrap();
		assert_eq!(json, decoded_json);
	}
}
