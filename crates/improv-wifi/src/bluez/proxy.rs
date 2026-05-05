//! Client proxies for BlueZ services we call out to.

use std::collections::HashMap;

use zbus::{
	proxy,
	zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

#[proxy(
	interface = "org.bluez.Adapter1",
	default_service = "org.bluez",
	gen_blocking = false
)]
pub(crate) trait Adapter1 {
	#[zbus(property)]
	fn powered(&self) -> zbus::Result<bool>;

	#[zbus(property)]
	fn set_powered(&self, powered: bool) -> zbus::Result<()>;

	#[zbus(property)]
	fn address(&self) -> zbus::Result<String>;
}

#[proxy(
	interface = "org.bluez.GattManager1",
	default_service = "org.bluez",
	gen_blocking = false
)]
pub(crate) trait GattManager1 {
	fn register_application(
		&self,
		application: &ObjectPath<'_>,
		options: HashMap<&str, Value<'_>>,
	) -> zbus::Result<()>;

	fn unregister_application(&self, application: &ObjectPath<'_>) -> zbus::Result<()>;
}

#[proxy(
	interface = "org.bluez.LEAdvertisingManager1",
	default_service = "org.bluez",
	gen_blocking = false
)]
pub(crate) trait LEAdvertisingManager1 {
	fn register_advertisement(
		&self,
		advertisement: &ObjectPath<'_>,
		options: HashMap<&str, Value<'_>>,
	) -> zbus::Result<()>;

	fn unregister_advertisement(&self, advertisement: &ObjectPath<'_>) -> zbus::Result<()>;
}

#[proxy(
	interface = "org.freedesktop.DBus.ObjectManager",
	default_service = "org.bluez",
	default_path = "/",
	gen_blocking = false
)]
pub(crate) trait BluezObjectManager {
	fn get_managed_objects(
		&self,
	) -> zbus::Result<HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>>;
}
