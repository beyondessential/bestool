#![cfg(target_os = "linux")]
//! An implementation of [improv-wifi] for Linux.
//!
//! This crate provides an implementation of the Improv Wi-Fi configuration protocol, as a
//! peripheral, via BlueZ's D-Bus API. It is intended to be used in conjunction with the
//! [improv-wifi] tooling for Web and Android, to allow for easy connection of an embedded Linux
//! device to a Wi-Fi network without the need for a display or other input peripherals.
//!
//! As there are many different network systems available for Linux, this crate supplies a trait to
//! allow any network configuration framework to be supported.
//!
//! If you are looking for an out-of-the-box solution, you can use the `improv-wifi-cli` application
//! crate, which uses this crate to provide a command-line program and systemd service.
//!
//! # Examples
//!
//! # Features
//!
//! - `miette`: Implements `miette::Diagnostic` on the error type.
//! - `networkmanager`: Enables the `NetworkManager` wifi configurator.
//!
//! [improv-wifi]: https://www.improv-wifi.com
//! [NetworkManager]: https://www.networkmanager.dev
