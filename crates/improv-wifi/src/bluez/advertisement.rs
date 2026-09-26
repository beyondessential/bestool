//! Server-side `org.bluez.LEAdvertisement1` interface.
//!
//! BlueZ calls `Release` once it stops advertising us. We treat that as informational only —
//! tear-down on our side is driven by the service's `run` loop dropping the registration handle.

use std::collections::HashMap;

use tracing::debug;
use zbus::interface;

#[derive(Debug, Clone)]
pub(crate) struct Advertisement {
	pub(crate) advertisement_type: String,
	pub(crate) service_uuids: Vec<String>,
	pub(crate) service_data: HashMap<String, Vec<u8>>,
	pub(crate) local_name: Option<String>,
	pub(crate) discoverable: bool,
}

#[interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
	#[zbus(property, name = "Type")]
	fn ty(&self) -> &str {
		&self.advertisement_type
	}

	// NOTE: property name is case sensitive
	#[zbus(property, name = "ServiceUUIDs")]
	fn service_uuids(&self) -> Vec<String> {
		self.service_uuids.clone()
	}

	#[zbus(property)]
	fn service_data(&self) -> HashMap<String, zbus::zvariant::Value<'static>> {
		self.service_data
			.iter()
			.map(|(uuid, data)| (uuid.clone(), zbus::zvariant::Value::from(data.clone())))
			.collect()
	}

	#[zbus(property)]
	fn local_name(&self) -> String {
		self.local_name.clone().unwrap_or_default()
	}

	#[zbus(property)]
	fn discoverable(&self) -> bool {
		self.discoverable
	}

	fn release(&self) {
		debug!("LEAdvertisement1.Release called by BlueZ");
	}
}
