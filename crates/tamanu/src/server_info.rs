//! Shared helpers for collecting Tamanu server identity and host facts.
//!
//! Used by `canopy register`, `alertd`, and `doctor`. Kept here so each
//! subcommand pulls from one place rather than reaching into a sibling's module.

use std::{
	collections::BTreeMap,
	path::{Path, PathBuf},
};

use miette::{IntoDiagnostic, Result, WrapErr as _};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Standard on-disk location for the Tamanu device key PEM.
///
/// - Linux: `/etc/tamanu/device-key.pem`
/// - Windows: `C:\Tamanu\device-key.pem`
/// - Other platforms: same as Linux.
///
/// The canopy registration is the source of truth for the device key; this
/// file is the plaintext fallback consulted when no registration is present.
pub fn standard_device_key_path() -> PathBuf {
	if cfg!(windows) {
		PathBuf::from(r"C:\Tamanu\device-key.pem")
	} else {
		PathBuf::from("/etc/tamanu/device-key.pem")
	}
}

/// Standard on-disk location for the Tamanu meta-server ID.
///
/// - Linux: `/etc/tamanu/server-id`
/// - Windows: `C:\Tamanu\server-id`
/// - Other platforms: same as Linux.
///
/// Like the device key, the canopy registration is the preferred source and
/// this file is the fallback.
pub fn standard_server_id_path() -> PathBuf {
	if cfg!(windows) {
		PathBuf::from(r"C:\Tamanu\server-id")
	} else {
		PathBuf::from("/etc/tamanu/server-id")
	}
}

/// Legacy on-disk location for cached canopy tags.
///
/// The cache now lives alongside the canopy registration
/// (`bestool_canopy::registration::default_tags_path`); this path is retained
/// as a read fallback for hosts that haven't refreshed their tags since the
/// move.
pub fn standard_tags_path() -> PathBuf {
	if cfg!(windows) {
		PathBuf::from(r"C:\Tamanu\tags.json")
	} else {
		PathBuf::from("/etc/tamanu/tags.json")
	}
}

/// Load the cached canopy tags written by `bestool tamanu tags`.
///
/// Reads [`standard_tags_path`] and returns the `tags` map. The cache is a
/// `{ "tags": { .. }, "fetched_at": .. }` object; only the `tags` field is read
/// here, so the timestamp and any future fields are ignored. Returns `None`
/// when the cache is absent or unparseable.
///
/// Used by callers that need the tags without a canopy round-trip — e.g. the
/// doctor reconciling `billing.*` tags against the instance's IMDS tags.
pub fn load_cached_tags() -> Option<BTreeMap<String, String>> {
	load_cached_tags_at(&standard_tags_path())
}

fn load_cached_tags_at(path: &Path) -> Option<BTreeMap<String, String>> {
	let bytes = match std::fs::read(path) {
		Ok(bytes) => bytes,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
		Err(err) => {
			debug!(path = %path.display(), %err, "could not read tags cache");
			return None;
		}
	};

	let value: serde_json::Value = match serde_json::from_slice(&bytes) {
		Ok(value) => value,
		Err(err) => {
			debug!(path = %path.display(), %err, "could not parse tags cache");
			return None;
		}
	};

	let obj = value.get("tags")?.as_object()?;
	Some(
		obj.iter()
			.filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
			.collect(),
	)
}

/// Resolve the `metaServerId` for this Tamanu server.
///
/// Resolution order:
/// 1. The canopy registration's `server_id`, when present.
/// 2. [`standard_server_id_path`], when present.
/// 3. Otherwise mint a fresh UUIDv4 and persist it to the file path.
///
/// Works without a DB connection throughout, so callers like the doctor daemon
/// can report status to canopy even when postgres is down (which is precisely
/// when canopy most needs to hear from us). Returns an error only when a fresh
/// ID must be minted but the file can't be written.
pub async fn get_or_create_server_id() -> Result<String> {
	get_or_create_server_id_at(&standard_server_id_path()).await
}

/// Test-shimmed core of [`get_or_create_server_id`] — same contract, with
/// the file path injected so unit tests can drive it without touching
/// `/etc/tamanu`.
async fn get_or_create_server_id_at(path: &Path) -> Result<String> {
	#[cfg(feature = "canopy-registration")]
	if let Some(reg) = load_registration().await
		&& let Some(id) = reg.server_id
	{
		return Ok(id);
	}

	if let Some(id) = read_server_id_file(path) {
		return Ok(id);
	}

	let id = Uuid::new_v4().to_string();
	info!(server_id = %id, "generating new metaServerId");
	write_server_id_file(path, &id)
		.into_diagnostic()
		.wrap_err_with(|| format!("persisting new metaServerId to {}", path.display()))?;
	Ok(id)
}

fn read_server_id_file(path: &Path) -> Option<String> {
	match std::fs::read_to_string(path) {
		Ok(s) => {
			let trimmed = s.trim();
			if trimmed.is_empty() {
				warn!(path = %path.display(), "server-id file is empty; ignoring");
				return None;
			}
			if Uuid::parse_str(trimmed).is_err() {
				warn!(
					path = %path.display(),
					"server-id file does not contain a UUID; ignoring",
				);
				return None;
			}
			debug!(path = %path.display(), server_id = trimmed, "loaded metaServerId from standard path");
			Some(trimmed.to_string())
		}
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
		Err(err) => {
			debug!(path = %path.display(), %err, "could not read server-id file");
			None
		}
	}
}

fn write_server_id_file(path: &Path, id: &str) -> std::io::Result<()> {
	use std::io::Write as _;

	if let Some(parent) = path.parent()
		&& !parent.exists()
	{
		return Err(std::io::Error::new(
			std::io::ErrorKind::NotFound,
			"parent directory does not exist",
		));
	}

	let tmp = path.with_extension("tmp");
	{
		let mut f = std::fs::OpenOptions::new()
			.write(true)
			.create_new(true)
			.open(&tmp)?;
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt as _;
			let perms = std::fs::Permissions::from_mode(0o644);
			f.set_permissions(perms)?;
		}
		f.write_all(id.as_bytes())?;
		f.write_all(b"\n")?;
		f.sync_all()?;
	}
	std::fs::rename(&tmp, path)
}

/// Generate a fresh P-256 device key as a PKCS#8 PEM.
///
/// `canopy register` calls this to mint the machine's mTLS identity when it
/// has none yet; the key is then kept inside the encrypted canopy registration
/// rather than written to a standalone file. Safe in the operator-first flow:
/// the operator creates the server record in canopy first, and register
/// publishes the public key over mTLS, so a freshly minted key is bound rather
/// than stranded.
#[cfg(feature = "device-key")]
pub fn generate_device_key_pem() -> Result<String> {
	use miette::miette;
	use p256::{
		SecretKey,
		elliptic_curve::rand_core::OsRng,
		pkcs8::{EncodePrivateKey as _, LineEnding},
	};

	let key = SecretKey::random(&mut OsRng);
	let pem = key
		.to_pkcs8_pem(LineEnding::LF)
		.map_err(|e| miette!("failed to encode generated deviceKey: {e}"))?;
	Ok(pem.to_string())
}

/// Load this host's canopy registration, logging (and swallowing) errors.
///
/// The registration is the source of truth for the device key and server id;
/// callers fall back to the standard plaintext file path when it's absent.
#[cfg(feature = "canopy-registration")]
async fn load_registration() -> Option<bestool_canopy::registration::Registration> {
	match bestool_canopy::registration::load().await {
		Ok(opt) => opt,
		Err(err) => {
			warn!(%err, "could not load canopy registration; falling back to legacy paths");
			None
		}
	}
}

/// Best-effort device key from the canopy registration or the standard file
/// path (no DB). Used by callers that degrade cleanly when it's absent.
pub async fn fetch_device_key() -> Option<String> {
	#[cfg(feature = "canopy-registration")]
	if let Some(reg) = load_registration().await
		&& let Some(key) = reg.device_key
	{
		return Some(key);
	}

	read_device_key_file(&standard_device_key_path())
}

/// Query the central server's `settings` table for `features.patientPortal`.
///
/// Tamanu mounts the patient portal API conditionally on this flag (see
/// `packages/central-server/app/createApi.js`), so a `true` value here is the
/// authoritative "this deployment runs the portal" signal — independent of
/// the ansible-side `PatientPortalFQDN` tag or the unused `patientPortal.portalUrl`
/// config field.
///
/// Settings are stored as one row per dotted leaf path, with the value column
/// as `JSONB`. The default if the row is absent is `false` (per the global
/// schema in `packages/settings/src/schema/global.ts`), so missing rows
/// resolve to `Some(false)`.
///
/// Returns `None` only when the query itself failed — that's the
/// "DB unreachable" / "transient SQL error" case, kept distinct from
/// `Some(false)` so callers can emit an Unknown expectation rather than
/// silently treating outage as opt-out.
pub async fn query_patient_portal_enabled(client: &tokio_postgres::Client) -> Option<bool> {
	match client
		.query_opt(
			"SELECT value FROM settings WHERE key = 'features.patientPortal' LIMIT 1",
			&[],
		)
		.await
	{
		Ok(Some(row)) => Some(
			row.try_get::<_, serde_json::Value>(0)
				.ok()
				.and_then(|v| v.as_bool())
				.unwrap_or(false),
		),
		Ok(None) => Some(false),
		Err(err) => {
			debug!(%err, "could not query features.patientPortal setting");
			None
		}
	}
}

fn read_device_key_file(path: &Path) -> Option<String> {
	match std::fs::read_to_string(path) {
		Ok(s) if !s.trim().is_empty() => {
			debug!(path = %path.display(), "loaded deviceKey from standard path");
			Some(s)
		}
		Ok(_) => {
			warn!(path = %path.display(), "deviceKey file is empty; ignoring");
			None
		}
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
		Err(err) => {
			debug!(path = %path.display(), %err, "could not read deviceKey file");
			None
		}
	}
}

/// Read the tailscale Self node's first IP and DNS name, if tailscale is
/// installed and responding to `status --json`.
pub async fn get_tailscale_info() -> (Option<String>, Option<String>) {
	let output = match tokio::process::Command::new("tailscale")
		.arg("status")
		.arg("--json")
		.output()
		.await
	{
		Ok(output) if output.status.success() => output,
		Ok(output) => {
			debug!(
				status = %output.status,
				"tailscale status command failed"
			);
			return (None, None);
		}
		Err(e) => {
			debug!(error = %e, "tailscale not available");
			return (None, None);
		}
	};

	let parsed: serde_json::Value = match serde_json::from_slice(&output.stdout) {
		Ok(v) => v,
		Err(e) => {
			warn!(error = %e, "failed to parse tailscale status JSON");
			return (None, None);
		}
	};

	let self_node = &parsed["Self"];
	let ip = self_node["TailscaleIPs"]
		.as_array()
		.and_then(|ips| ips.first())
		.and_then(|ip| ip.as_str())
		.map(String::from);
	let name = self_node["DNSName"]
		.as_str()
		.map(|s| s.trim_end_matches('.').to_string());

	(ip, name)
}

/// Detect the host's installed Node.js version by running `node --version`.
///
/// Returns `None` if node isn't on `PATH` or the command fails. The leading
/// `v` that node prints (e.g. `v20.11.0`) is stripped, so the value is a bare
/// version string.
pub async fn detect_node_version() -> Option<String> {
	let output = tokio::process::Command::new("node")
		.arg("--version")
		.output()
		.await
		.ok()?;
	if !output.status.success() {
		debug!(status = %output.status, "node --version failed");
		return None;
	}

	parse_node_version(&String::from_utf8_lossy(&output.stdout))
}

/// Parse the output of `node --version` (e.g. `v20.11.0\n`) into a bare version
/// string. Returns `None` for empty/whitespace-only input.
fn parse_node_version(raw: &str) -> Option<String> {
	let version = raw.trim().trim_start_matches('v');
	if version.is_empty() {
		None
	} else {
		Some(version.to_string())
	}
}

/// Linux: read `systemd-detect-virt`'s output. Returns `None` if the command
/// is unavailable. The string is whatever systemd reports (e.g. `kvm`, `lxc`,
/// `none` for bare metal).
pub async fn detect_virtualisation() -> Option<String> {
	let output = tokio::process::Command::new("systemd-detect-virt")
		.output()
		.await
		.ok()?;

	let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
	if stdout.is_empty() {
		return None;
	}

	Some(stdout)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn server_id_resolves_from_file() {
		// On a provisioned host the standard file path holds the id; resolution
		// reads it straight back without minting a new one.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("server-id");
		let cached = uuid::Uuid::new_v4().to_string();
		std::fs::write(&path, &cached).unwrap();

		let id = get_or_create_server_id_at(&path).await.unwrap();
		assert_eq!(id, cached);
	}

	#[tokio::test]
	async fn server_id_mints_and_persists_when_absent() {
		// Brand-new host with no file: mint a fresh UUID and write it to the
		// file so the next run reads the same id back.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("server-id");

		let id = get_or_create_server_id_at(&path).await.unwrap();
		uuid::Uuid::parse_str(&id).expect("minted id should be a UUID");
		assert_eq!(read_server_id_file(&path).as_deref(), Some(id.as_str()));

		let again = get_or_create_server_id_at(&path).await.unwrap();
		assert_eq!(again, id, "resolution must be stable across runs");
	}

	#[tokio::test]
	async fn server_id_errors_when_file_unwritable() {
		// No file and nowhere to write one (missing parent dir) — must surface
		// as an error rather than silently losing the minted id.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("nope").join("server-id");
		let err = get_or_create_server_id_at(&path)
			.await
			.expect_err("unwritable path → must error");
		let msg = format!("{err}");
		assert!(msg.contains("metaServerId"), "{msg}");
	}

	#[test]
	fn parse_node_version_strips_v_prefix_and_whitespace() {
		assert_eq!(parse_node_version("v20.11.0\n").as_deref(), Some("20.11.0"));
		assert_eq!(
			parse_node_version("  v18.19.1  ").as_deref(),
			Some("18.19.1")
		);
		assert_eq!(parse_node_version("20.11.0").as_deref(), Some("20.11.0"));
	}

	#[test]
	fn parse_node_version_treats_empty_as_none() {
		assert!(parse_node_version("").is_none());
		assert!(parse_node_version("  \n").is_none());
	}

	#[test]
	fn read_device_key_file_returns_none_for_missing() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("missing.pem");
		assert!(read_device_key_file(&path).is_none());
	}

	#[test]
	fn read_device_key_file_returns_contents() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("device.pem");
		std::fs::write(&path, "PEM CONTENT").unwrap();
		assert_eq!(read_device_key_file(&path).as_deref(), Some("PEM CONTENT"));
	}

	#[test]
	fn read_device_key_file_treats_empty_as_missing() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("empty.pem");
		std::fs::write(&path, "   \n").unwrap();
		assert!(read_device_key_file(&path).is_none());
	}

	#[test]
	fn load_cached_tags_reads_tags_object() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("tags.json");
		std::fs::write(
			&path,
			r#"{"tags":{"billing.customer":"acme","role":"central"},"fetched_at":"2026-01-01T00:00:00Z"}"#,
		)
		.unwrap();
		let tags = load_cached_tags_at(&path).expect("should load");
		assert_eq!(
			tags.get("billing.customer").map(String::as_str),
			Some("acme")
		);
		assert_eq!(tags.get("role").map(String::as_str), Some("central"));
	}

	#[test]
	fn load_cached_tags_none_for_missing_file() {
		let dir = tempfile::tempdir().unwrap();
		assert!(load_cached_tags_at(&dir.path().join("nope.json")).is_none());
	}

	#[test]
	fn load_cached_tags_none_for_garbage() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("tags.json");
		std::fs::write(&path, "not json").unwrap();
		assert!(load_cached_tags_at(&path).is_none());
	}

	#[test]
	fn load_cached_tags_empty_when_no_tags_field() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("tags.json");
		std::fs::write(&path, r#"{"fetched_at":null}"#).unwrap();
		assert!(load_cached_tags_at(&path).is_none());
	}

	#[test]
	fn read_server_id_file_returns_none_for_missing() {
		let dir = tempfile::tempdir().unwrap();
		assert!(read_server_id_file(&dir.path().join("missing")).is_none());
	}

	#[test]
	fn read_server_id_file_returns_uuid() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("server-id");
		let id = "7deb2793-0425-427e-8a19-7213946fa9be";
		std::fs::write(&path, format!("{id}\n")).unwrap();
		assert_eq!(read_server_id_file(&path).as_deref(), Some(id));
	}

	#[test]
	fn read_server_id_file_rejects_non_uuid() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("server-id");
		std::fs::write(&path, "not-a-uuid\n").unwrap();
		assert!(read_server_id_file(&path).is_none());
	}

	#[test]
	fn write_server_id_file_roundtrips() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("server-id");
		let id = Uuid::new_v4().to_string();
		write_server_id_file(&path, &id).unwrap();
		assert_eq!(read_server_id_file(&path).as_deref(), Some(id.as_str()));

		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt as _;
			let mode = std::fs::metadata(&path).unwrap().permissions().mode();
			assert_eq!(mode & 0o777, 0o644);
		}
	}

	#[cfg(feature = "device-key")]
	#[test]
	fn generate_device_key_pem_produces_valid_pkcs8() {
		use p256::{SecretKey, pkcs8::DecodePrivateKey as _};

		let pem = generate_device_key_pem().unwrap();
		assert!(pem.starts_with("-----BEGIN PRIVATE KEY-----"));
		assert!(pem.trim_end().ends_with("-----END PRIVATE KEY-----"));
		SecretKey::from_pkcs8_pem(&pem).expect("generated PEM must parse as P-256 PKCS8");
	}

	#[cfg(feature = "device-key")]
	#[test]
	fn generate_device_key_pem_is_non_deterministic() {
		let a = generate_device_key_pem().unwrap();
		let b = generate_device_key_pem().unwrap();
		assert_ne!(a, b);
	}

	#[cfg(feature = "device-key")]
	#[test]
	fn generate_device_key_pem_parses_as_p256() {
		use p256::{SecretKey, pkcs8::DecodePrivateKey as _};

		let pem = generate_device_key_pem().unwrap();
		SecretKey::from_pkcs8_pem(&pem).expect("returned PEM must parse as P-256 PKCS8");
	}
}
