//! An implementation of the [Improv Wi-Fi] BLE peripheral protocol for Linux.
//!
//! This crate provides the device side of the Improv Wi-Fi configuration protocol via BlueZ's
//! D-Bus API. It is intended for embedded Linux devices that need to be provisioned onto a Wi-Fi
//! network without a display or input peripherals.
//!
//! [Improv Wi-Fi]: https://www.improv-wifi.com
#![cfg(target_os = "linux")]
