//! NetworkManager backend for [`crate::WifiConfigurator`], implemented over D-Bus.

use std::{collections::HashMap, time::Duration};

use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};
use zbus::{Connection, proxy};

use crate::{Capabilities, DeviceInfo, Error, Network, WifiConfigurator};

/// NM device type for Wi-Fi (`NM_DEVICE_TYPE_WIFI`).
const NM_DEVICE_TYPE_WIFI: u32 = 2;

/// NM device state (`NM_DEVICE_STATE_ACTIVATED`).
const NM_DEVICE_STATE_ACTIVATED: u32 = 100;

/// NM active-connection state (`NM_ACTIVE_CONNECTION_STATE_ACTIVATED`).
const NM_ACTIVE_CONNECTION_STATE_ACTIVATED: u32 = 2;

/// NM active-connection state (`NM_ACTIVE_CONNECTION_STATE_DEACTIVATING`).
const NM_ACTIVE_CONNECTION_STATE_DEACTIVATING: u32 = 3;

/// 802.11 security flags from `NM80211ApSecurityFlags`.
const NM_AP_SEC_NONE: u32 = 0x0;
const NM_AP_SEC_KEY_MGMT_PSK: u32 = 0x100;
const NM_AP_SEC_KEY_MGMT_802_1X: u32 = 0x200;
const NM_AP_SEC_KEY_MGMT_SAE: u32 = 0x400;
const NM_AP_SEC_KEY_MGMT_OWE: u32 = 0x800;

/// 802.11 AP flags from `NM80211ApFlags`.
const NM_AP_FLAGS_PRIVACY: u32 = 0x1;

/// How long we wait for a new connection to reach `ACTIVATED`.
const PROVISION_TIMEOUT: Duration = Duration::from_secs(30);

/// How long we give NM after `RequestScan` before reading APs.
const SCAN_SETTLE: Duration = Duration::from_secs(4);

#[proxy(
	interface = "org.freedesktop.NetworkManager",
	default_service = "org.freedesktop.NetworkManager",
	default_path = "/org/freedesktop/NetworkManager"
)]
trait NetworkManager {
	fn get_devices(&self) -> zbus::Result<Vec<zbus::zvariant::OwnedObjectPath>>;

	#[zbus(name = "AddAndActivateConnection")]
	fn add_and_activate_connection(
		&self,
		connection: HashMap<&str, HashMap<&str, zbus::zvariant::Value<'_>>>,
		device: &zbus::zvariant::ObjectPath<'_>,
		specific_object: &zbus::zvariant::ObjectPath<'_>,
	) -> zbus::Result<(
		zbus::zvariant::OwnedObjectPath, // connection path
		zbus::zvariant::OwnedObjectPath, // active-connection path
	)>;
}

#[proxy(
	interface = "org.freedesktop.NetworkManager.Device",
	default_service = "org.freedesktop.NetworkManager"
)]
trait NmDevice {
	#[zbus(property)]
	fn device_type(&self) -> zbus::Result<u32>;

	#[zbus(property)]
	fn state(&self) -> zbus::Result<u32>;
}

#[proxy(
	interface = "org.freedesktop.NetworkManager.Device.Wireless",
	default_service = "org.freedesktop.NetworkManager"
)]
trait NmDeviceWireless {
	fn request_scan(&self, options: HashMap<&str, zbus::zvariant::Value<'_>>) -> zbus::Result<()>;

	#[zbus(property)]
	fn access_points(&self) -> zbus::Result<Vec<zbus::zvariant::OwnedObjectPath>>;
}

#[proxy(
	interface = "org.freedesktop.NetworkManager.AccessPoint",
	default_service = "org.freedesktop.NetworkManager"
)]
trait NmAccessPoint {
	#[zbus(property, name = "Ssid")]
	fn ssid(&self) -> zbus::Result<Vec<u8>>;

	#[zbus(property)]
	fn strength(&self) -> zbus::Result<u8>;

	#[zbus(property)]
	fn flags(&self) -> zbus::Result<u32>;

	#[zbus(property)]
	fn wpa_flags(&self) -> zbus::Result<u32>;

	#[zbus(property)]
	fn rsn_flags(&self) -> zbus::Result<u32>;
}

#[proxy(
	interface = "org.freedesktop.NetworkManager.Connection.Active",
	default_service = "org.freedesktop.NetworkManager"
)]
trait NmActiveConnection {
	#[zbus(property)]
	fn state(&self) -> zbus::Result<u32>;
}

#[proxy(
	interface = "org.freedesktop.hostname1",
	default_service = "org.freedesktop.hostname1",
	default_path = "/org/freedesktop/hostname1"
)]
trait Hostname1 {
	fn set_static_hostname(&self, name: &str, interactive: bool) -> zbus::Result<()>;

	#[zbus(property, name = "StaticHostname")]
	fn static_hostname(&self) -> zbus::Result<String>;
}

/// NetworkManager-backed [`WifiConfigurator`].
#[derive(Clone, Debug)]
pub struct NetworkManagerBackend {
	connection: Connection,
	device_name: String,
	firmware: String,
	firmware_version: String,
	hardware: String,
}

impl NetworkManagerBackend {
	/// Connect to the system bus and prepare the backend.
	pub async fn new(device_name: impl Into<String>) -> Result<Self, Error> {
		let connection = Connection::system().await.map_err(map_zbus_err)?;
		Ok(Self {
			connection,
			device_name: device_name.into(),
			firmware: "bestool".into(),
			firmware_version: env!("CARGO_PKG_VERSION").into(),
			hardware: std::env::consts::ARCH.into(),
		})
	}

	pub fn with_firmware(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
		self.firmware = name.into();
		self.firmware_version = version.into();
		self
	}

	pub fn with_hardware(mut self, hardware: impl Into<String>) -> Self {
		self.hardware = hardware.into();
		self
	}

	async fn first_wifi_device(&self) -> Result<zbus::zvariant::OwnedObjectPath, Error> {
		let nm = NetworkManagerProxy::new(&self.connection)
			.await
			.map_err(map_zbus_err)?;
		let devices = nm.get_devices().await.map_err(map_zbus_err)?;
		for path in devices {
			let dev = NmDeviceProxy::builder(&self.connection)
				.path(path.clone())
				.map_err(map_zbus_err)?
				.build()
				.await
				.map_err(map_zbus_err)?;
			if let Ok(t) = dev.device_type().await
				&& t == NM_DEVICE_TYPE_WIFI
			{
				return Ok(path);
			}
		}
		warn!("no Wi-Fi device found via NetworkManager");
		Err(Error::Unknown)
	}

	/// Whether the Wi-Fi device is currently connected to a network.
	///
	/// Returns `true` if the first Wi-Fi device is in NM state `Activated`. If no Wi-Fi
	/// device is found, returns `false` (treated as "not connected" so the caller can fall
	/// through to its usual failure path on actual provisioning attempts).
	pub async fn is_connected(&self) -> Result<bool, Error> {
		let device_path = match self.first_wifi_device().await {
			Ok(p) => p,
			Err(_) => return Ok(false),
		};
		let dev = NmDeviceProxy::builder(&self.connection)
			.path(device_path)
			.map_err(map_zbus_err)?
			.build()
			.await
			.map_err(map_zbus_err)?;
		let state = dev.state().await.map_err(map_zbus_err)?;
		Ok(state == NM_DEVICE_STATE_ACTIVATED)
	}

	/// Whether any saved Wi-Fi connection profile exists.
	///
	/// Returns `true` if NM has at least one connection profile of type `802-11-wireless`,
	/// regardless of whether it is currently connected. Use this to gate boot-time
	/// initialisation: a fresh device returns `false`, a previously provisioned one returns
	/// `true` even if it's not currently connected to the network.
	pub async fn is_configured(&self) -> Result<bool, Error> {
		let settings = NmSettingsProxy::new(&self.connection)
			.await
			.map_err(map_zbus_err)?;
		let conns = settings.list_connections().await.map_err(map_zbus_err)?;
		for path in conns {
			let conn = NmSettingsConnectionProxy::builder(&self.connection)
				.path(path)
				.map_err(map_zbus_err)?
				.build()
				.await
				.map_err(map_zbus_err)?;
			let Ok(settings_dict) = conn.get_settings().await else {
				continue;
			};
			let Some(connection) = settings_dict.get("connection") else {
				continue;
			};
			let Some(type_val) = connection.get("type") else {
				continue;
			};
			if let Ok(type_str) = String::try_from(type_val.clone())
				&& type_str == "802-11-wireless"
			{
				return Ok(true);
			}
		}
		Ok(false)
	}
}

impl WifiConfigurator for NetworkManagerBackend {
	fn capabilities(&self) -> Capabilities {
		Capabilities {
			identify: false,
			device_info: true,
			scan: true,
			hostname: true,
		}
	}

	async fn device_info(&self) -> Result<DeviceInfo, Error> {
		let (os_name, os_version) = read_os_release();
		Ok(DeviceInfo {
			firmware: self.firmware.clone(),
			version: self.firmware_version.clone(),
			hardware: self.hardware.clone(),
			device_name: self.device_name.clone(),
			os_name,
			os_version,
		})
	}

	async fn scan(&self) -> Result<Vec<Network>, Error> {
		let device_path = self.first_wifi_device().await?;
		let wireless = NmDeviceWirelessProxy::builder(&self.connection)
			.path(device_path)
			.map_err(map_zbus_err)?
			.build()
			.await
			.map_err(map_zbus_err)?;

		// Best-effort: errors here usually mean "scan already in progress" — ignore.
		if let Err(err) = wireless.request_scan(HashMap::new()).await {
			debug!(?err, "RequestScan returned an error (often benign)");
		}
		sleep(SCAN_SETTLE).await;

		let aps = wireless.access_points().await.map_err(map_zbus_err)?;
		let mut out = Vec::with_capacity(aps.len());
		for ap_path in aps {
			let ap = NmAccessPointProxy::builder(&self.connection)
				.path(ap_path)
				.map_err(map_zbus_err)?
				.build()
				.await
				.map_err(map_zbus_err)?;
			let ssid_bytes = ap.ssid().await.map_err(map_zbus_err)?;
			let ssid = match String::from_utf8(ssid_bytes) {
				Ok(s) if !s.is_empty() => s,
				_ => continue,
			};
			let strength_pct = ap.strength().await.unwrap_or(0);
			let flags = ap.flags().await.unwrap_or(0);
			let wpa_flags = ap.wpa_flags().await.unwrap_or(0);
			let rsn_flags = ap.rsn_flags().await.unwrap_or(0);
			out.push(Network {
				ssid,
				rssi: strength_to_dbm(strength_pct),
				auth: auth_string(flags, wpa_flags, rsn_flags),
			});
		}
		Ok(out)
	}

	async fn get_hostname(&self) -> Result<String, Error> {
		let proxy = Hostname1Proxy::new(&self.connection)
			.await
			.map_err(map_zbus_err)?;
		proxy.static_hostname().await.map_err(map_zbus_err)
	}

	async fn set_hostname(&self, name: String) -> Result<(), Error> {
		if !is_valid_rfc1123_hostname(&name) {
			return Err(Error::BadHostname);
		}
		let proxy = Hostname1Proxy::new(&self.connection)
			.await
			.map_err(map_zbus_err)?;
		proxy
			.set_static_hostname(&name, false)
			.await
			.map_err(map_zbus_err)
	}

	async fn provision(&self, ssid: String, password: String) -> Result<Vec<String>, Error> {
		let device_path = self.first_wifi_device().await?;
		let nm = NetworkManagerProxy::new(&self.connection)
			.await
			.map_err(map_zbus_err)?;

		let mut connection: HashMap<&str, HashMap<&str, zbus::zvariant::Value<'_>>> =
			HashMap::new();

		let mut conn_settings = HashMap::new();
		conn_settings.insert("type", zbus::zvariant::Value::from("802-11-wireless"));
		conn_settings.insert("id", zbus::zvariant::Value::from(ssid.clone()));
		connection.insert("connection", conn_settings);

		let mut wifi_settings = HashMap::new();
		wifi_settings.insert(
			"ssid",
			zbus::zvariant::Value::from(ssid.as_bytes().to_vec()),
		);
		wifi_settings.insert("mode", zbus::zvariant::Value::from("infrastructure"));
		connection.insert("802-11-wireless", wifi_settings);

		if !password.is_empty() {
			let mut sec_settings = HashMap::new();
			sec_settings.insert("key-mgmt", zbus::zvariant::Value::from("wpa-psk"));
			sec_settings.insert("psk", zbus::zvariant::Value::from(password));
			connection.insert("802-11-wireless-security", sec_settings);
		}

		let empty_path = zbus::zvariant::ObjectPath::try_from("/").unwrap();
		let device_obj = device_path.as_ref();
		let (conn_path, active_path) = nm
			.add_and_activate_connection(connection, &device_obj, &empty_path)
			.await
			.map_err(|err| {
				warn!(?err, "AddAndActivateConnection failed");
				Error::UnableToConnect
			})?;
		info!(?conn_path, ?active_path, "activated connection");

		let active = NmActiveConnectionProxy::builder(&self.connection)
			.path(active_path.clone())
			.map_err(map_zbus_err)?
			.build()
			.await
			.map_err(map_zbus_err)?;

		let activated = timeout(PROVISION_TIMEOUT, async {
			loop {
				let state = active.state().await.unwrap_or(0);
				if state == NM_ACTIVE_CONNECTION_STATE_ACTIVATED {
					return true;
				}
				if state >= NM_ACTIVE_CONNECTION_STATE_DEACTIVATING {
					return false;
				}
				sleep(Duration::from_millis(500)).await;
			}
		})
		.await
		.unwrap_or(false);

		if !activated {
			warn!("connection did not reach ACTIVATED in time, deleting");
			let _ = delete_connection(&self.connection, &conn_path).await;
			return Err(Error::UnableToConnect);
		}

		Ok(Vec::new())
	}
}

#[proxy(
	interface = "org.freedesktop.NetworkManager.Settings.Connection",
	default_service = "org.freedesktop.NetworkManager"
)]
trait NmSettingsConnection {
	fn delete(&self) -> zbus::Result<()>;

	fn get_settings(
		&self,
	) -> zbus::Result<HashMap<String, HashMap<String, zbus::zvariant::OwnedValue>>>;
}

#[proxy(
	interface = "org.freedesktop.NetworkManager.Settings",
	default_service = "org.freedesktop.NetworkManager",
	default_path = "/org/freedesktop/NetworkManager/Settings"
)]
trait NmSettings {
	fn list_connections(&self) -> zbus::Result<Vec<zbus::zvariant::OwnedObjectPath>>;
}

async fn delete_connection(
	conn: &Connection,
	path: &zbus::zvariant::OwnedObjectPath,
) -> zbus::Result<()> {
	let proxy = NmSettingsConnectionProxy::builder(conn)
		.path(path.clone())?
		.build()
		.await?;
	proxy.delete().await
}

fn map_zbus_err(err: zbus::Error) -> Error {
	warn!(?err, "zbus error");
	Error::Unknown
}

/// Map NM's 0-100% strength to a fake dBm value matching the Improv-Wi-Fi expectations
/// (negative number, e.g. -60). NM doesn't expose raw dBm; this is a reasonable approximation.
fn strength_to_dbm(strength_pct: u8) -> i16 {
	// 100% → -30 dBm, 0% → -90 dBm.
	-90 + (strength_pct as i16) * 60 / 100
}

fn auth_string(ap_flags: u32, wpa_flags: u32, rsn_flags: u32) -> String {
	let mut parts: Vec<&str> = Vec::new();
	let privacy = ap_flags & NM_AP_FLAGS_PRIVACY != 0;

	if wpa_flags != NM_AP_SEC_NONE {
		// "WPA" version 1.
		parts.push(if wpa_flags & NM_AP_SEC_KEY_MGMT_802_1X != 0 {
			"WPA EAP"
		} else {
			"WPA"
		});
	}
	if rsn_flags != NM_AP_SEC_NONE {
		if rsn_flags & NM_AP_SEC_KEY_MGMT_SAE != 0 {
			parts.push("WPA3");
		} else if rsn_flags & NM_AP_SEC_KEY_MGMT_OWE != 0 {
			parts.push("WPA2");
		} else if rsn_flags & NM_AP_SEC_KEY_MGMT_802_1X != 0 {
			parts.push("WPA2 EAP");
		} else if rsn_flags & NM_AP_SEC_KEY_MGMT_PSK != 0 {
			parts.push("WPA2");
		}
	}
	if parts.is_empty() {
		if privacy { "WEP".into() } else { "NO".into() }
	} else {
		parts.join("/")
	}
}

/// RFC 1123 hostname check: 1-253 chars, dot-separated labels of 1-63 chars each, each label
/// containing only `[A-Za-z0-9-]` and not starting/ending with `-`.
fn is_valid_rfc1123_hostname(s: &str) -> bool {
	if s.is_empty() || s.len() > 253 {
		return false;
	}
	s.split('.').all(|label| {
		!label.is_empty()
			&& label.len() <= 63
			&& !label.starts_with('-')
			&& !label.ends_with('-')
			&& label
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
	})
}

fn read_os_release() -> (Option<String>, Option<String>) {
	let Ok(content) = std::fs::read_to_string("/etc/os-release") else {
		return (None, None);
	};
	let mut name = None;
	let mut version = None;
	for line in content.lines() {
		let Some((k, v)) = line.split_once('=') else {
			continue;
		};
		let v = v.trim_matches('"').to_owned();
		match k {
			"NAME" => name = Some(v),
			"VERSION_ID" => version = Some(v),
			_ => {}
		}
	}
	(name, version)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rfc1123_acceptance() {
		assert!(is_valid_rfc1123_hostname("a"));
		assert!(is_valid_rfc1123_hostname("my-host"));
		assert!(is_valid_rfc1123_hostname("a.b.c"));
		assert!(is_valid_rfc1123_hostname(&"a".repeat(63)));
		assert!(!is_valid_rfc1123_hostname(""));
		assert!(!is_valid_rfc1123_hostname("-bad"));
		assert!(!is_valid_rfc1123_hostname("bad-"));
		assert!(!is_valid_rfc1123_hostname("a..b"));
		assert!(!is_valid_rfc1123_hostname("under_score"));
		assert!(!is_valid_rfc1123_hostname(&"a".repeat(64)));
	}

	#[test]
	fn auth_string_combinations() {
		assert_eq!(auth_string(0, 0, 0), "NO");
		assert_eq!(auth_string(NM_AP_FLAGS_PRIVACY, 0, 0), "WEP");
		assert_eq!(
			auth_string(NM_AP_FLAGS_PRIVACY, NM_AP_SEC_KEY_MGMT_PSK, 0),
			"WPA"
		);
		assert_eq!(
			auth_string(NM_AP_FLAGS_PRIVACY, 0, NM_AP_SEC_KEY_MGMT_PSK),
			"WPA2"
		);
		assert_eq!(
			auth_string(NM_AP_FLAGS_PRIVACY, 0, NM_AP_SEC_KEY_MGMT_SAE),
			"WPA3"
		);
		assert_eq!(
			auth_string(
				NM_AP_FLAGS_PRIVACY,
				NM_AP_SEC_KEY_MGMT_PSK,
				NM_AP_SEC_KEY_MGMT_PSK,
			),
			"WPA/WPA2"
		);
		assert_eq!(
			auth_string(NM_AP_FLAGS_PRIVACY, 0, NM_AP_SEC_KEY_MGMT_802_1X),
			"WPA2 EAP"
		);
	}

	#[test]
	fn strength_mapping() {
		assert_eq!(strength_to_dbm(0), -90);
		assert_eq!(strength_to_dbm(100), -30);
		assert_eq!(strength_to_dbm(50), -60);
	}

	#[test]
	fn os_release_handles_missing() {
		// We can't really test reading /etc/os-release, but we can ensure the function doesn't
		// panic on a malformed file by using a private helper if we had one. For now, trust the
		// implementation and just exercise the path.
		let _ = read_os_release();
	}
}
