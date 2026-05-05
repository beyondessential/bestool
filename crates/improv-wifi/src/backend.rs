use std::future::Future;

use crate::{Capabilities, Error};

/// Information returned by the Device Info command (`0x03`).
///
/// Per spec, the RPC Result for Device Info is a list of strings: firmware name, firmware version,
/// hardware identifier, device name, plus optionally OS name and OS version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeviceInfo {
	pub firmware: String,
	pub version: String,
	pub hardware: String,
	pub device_name: String,
	pub os_name: Option<String>,
	pub os_version: Option<String>,
}

impl DeviceInfo {
	/// Encode as the string list expected by the Improv-Wi-Fi RPC Result format.
	pub fn into_strings(self) -> Vec<String> {
		let mut out = vec![self.firmware, self.version, self.hardware, self.device_name];
		if let Some(os) = self.os_name {
			out.push(os);
			out.push(self.os_version.unwrap_or_default());
		}
		out
	}
}

/// One Wi-Fi network discovered by [`WifiConfigurator::scan`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Network {
	pub ssid: String,
	/// RSSI in dBm (e.g. `-60`).
	pub rssi: i16,
	/// Auth type string per Improv-Wi-Fi spec: `WEP`, `WPA`, `WPA2`, `WPA2 EAP`, `WPA3`, `WAPI`,
	/// `NO`. Combine multiple values with `/` (e.g. `WPA/WPA2`).
	pub auth: String,
}

/// A backend that knows how to talk to the host's network stack.
///
/// Implementations live in this crate (`networkmanager` feature) or downstream. The Improv-Wi-Fi
/// service drives this trait in response to RPC commands.
///
/// All methods take `&self` so the backend can be shared across BLE callbacks; implementations are
/// expected to use interior mutability (or to be stateless).
pub trait WifiConfigurator: Send + Sync + 'static {
	/// Capability bits for the Capabilities characteristic.
	fn capabilities(&self) -> Capabilities;

	/// Make the device perform a visual or audible signal. Default: no-op.
	fn identify(&self) -> impl Future<Output = Result<(), Error>> + Send {
		async { Ok(()) }
	}

	/// Return device info. Required if [`Capabilities::device_info`] is set.
	fn device_info(&self) -> impl Future<Output = Result<DeviceInfo, Error>> + Send;

	/// List visible Wi-Fi networks. Required if [`Capabilities::scan`] is set.
	fn scan(&self) -> impl Future<Output = Result<Vec<Network>, Error>> + Send;

	/// Get the current hostname. Required if [`Capabilities::hostname`] is set.
	fn get_hostname(&self) -> impl Future<Output = Result<String, Error>> + Send;

	/// Set the hostname. Required if [`Capabilities::hostname`] is set.
	///
	/// Implementations should validate against RFC 1123 and return [`Error::BadHostname`] on
	/// failure.
	fn set_hostname(&self, name: String) -> impl Future<Output = Result<(), Error>> + Send;

	/// Get the device name. The Improv-Wi-Fi spec uses the same capability bit as hostname for
	/// this command. Default: returns the same value as `device_info().device_name`.
	fn get_device_name(&self) -> impl Future<Output = Result<String, Error>> + Send {
		async { self.device_info().await.map(|i| i.device_name) }
	}

	/// Set the device name. Default: returns [`Error::Unknown`]; override if supported.
	fn set_device_name(&self, _name: String) -> impl Future<Output = Result<(), Error>> + Send {
		async { Err(Error::Unknown) }
	}

	/// Provision the device with the given Wi-Fi credentials.
	///
	/// On success, returns a list of strings to include in the RPC Result — typically a single
	/// URL the client can redirect to (e.g. `http://192.0.2.42`). The list may be empty.
	fn provision(
		&self,
		ssid: String,
		password: String,
	) -> impl Future<Output = Result<Vec<String>, Error>> + Send;
}
