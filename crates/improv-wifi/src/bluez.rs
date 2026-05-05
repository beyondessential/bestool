//! BlueZ peripheral implementation backed by `zbus`.
//!
//! We expose ourselves to BlueZ as a tree of D-Bus objects rooted at [`APP_PATH`]:
//!
//! - `<root>` exports `org.freedesktop.DBus.ObjectManager` (provided by zbus when an interface
//!   is registered at any descendant path).
//! - `<root>/service0` is a single primary GATT service.
//! - `<root>/service0/charN` are the five Improv-Wi-Fi characteristics.
//! - `<root>/adv0` is the BLE advertisement object that we register with BlueZ's
//!   `LEAdvertisingManager1`.
//!
//! BlueZ then drives `ReadValue`/`WriteValue`/`StartNotify`/`StopNotify` on the characteristics
//! and reads our properties via the standard `org.freedesktop.DBus.Properties` interface.

mod advertisement;
mod app;
mod gatt;
mod proxy;

pub(crate) use app::{AppHandles, find_adapter, install, power_on_adapter, run};
