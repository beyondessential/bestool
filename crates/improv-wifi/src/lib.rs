//! An implementation of [improv-wifi](https://www.improv-wifi.com) for Linux.
//!
//! This crate provides an implementation of the Improv Wi-Fi configuration protocol, as a
//! peripheral, via BlueZ's D-Bus API. It is intended to be used in conjunction with the
//! [improv-wifi](https://www.improv-wifi.com) tooling for Web and Android, to allow for easy
//! connection of an embedded Linux device to a Wi-Fi network without the need for a display or
//! other input peripherals.
//!
//! As there are many different network managers available for Linux, this crate provides a trait to
//! allow any network manager to be supported.
//!
//! It also bundles an implementation for [NetworkManager](https://www.networkmanager.dev) with the
//! optional feature `networkmanager`.
