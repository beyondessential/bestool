//! An implementation of the [Improv Wi-Fi] BLE peripheral protocol for Linux.
//!
//! This crate provides the device side of the Improv Wi-Fi configuration protocol via BlueZ's
//! D-Bus API. It is intended for embedded Linux devices that need to be provisioned onto a Wi-Fi
//! network without a display or input peripherals.
//!
//! [Improv Wi-Fi]: https://www.improv-wifi.com
#![cfg(target_os = "linux")]

use bluer::Uuid;

mod backend;
mod error;
pub mod rpc;
mod state;

pub use backend::{DeviceInfo, Network, WifiConfigurator};
pub use error::Error;
pub use state::{Capabilities, Status};

pub const SERVICE_UUID: Uuid = Uuid::from_u128(0x00467768_6228_2272_4663_277478268000);
pub const CHARACTERISTIC_UUID_CAPABILITIES: Uuid =
	Uuid::from_u128(0x00467768_6228_2272_4663_277478268005);
pub const CHARACTERISTIC_UUID_CURRENT_STATE: Uuid =
	Uuid::from_u128(0x00467768_6228_2272_4663_277478268001);
pub const CHARACTERISTIC_UUID_ERROR_STATE: Uuid =
	Uuid::from_u128(0x00467768_6228_2272_4663_277478268002);
pub const CHARACTERISTIC_UUID_RPC_COMMAND: Uuid =
	Uuid::from_u128(0x00467768_6228_2272_4663_277478268003);
pub const CHARACTERISTIC_UUID_RPC_RESULT: Uuid =
	Uuid::from_u128(0x00467768_6228_2272_4663_277478268004);

/// 16-bit Service Data UUID used in BLE advertisements (`0x4677`).
pub const ADVERTISEMENT_SERVICE_DATA_UUID: Uuid =
	Uuid::from_u128(0x00004677_0000_1000_8000_00805f9b34fb);
