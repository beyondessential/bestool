//! What the device tells a client about itself: its hostname and its addresses.
//!
//! This is the device-to-client direction of the channel demonstration. It stands in for the Iti's
//! LCD, and deliberately does not copy that screen's shape: the panel's limits are display limits,
//! not information limits, so the device sends every global address it has with the interface each
//! belongs to and lets the client decide what is worth showing.
//!
//! Loopback and link-local addresses are left out, because they are noise in any presentation.

use std::{fs, net::IpAddr};

use bliti_core::channel::messages::{Address, AddressFamily, DeviceMessage};

/// The device's hostname, from the kernel rather than from a configuration file, so it is what the
/// system actually answers to.
pub fn hostname() -> String {
	fs::read_to_string("/proc/sys/kernel/hostname")
		.map(|name| name.trim().to_owned())
		.unwrap_or_else(|_| "unknown".to_owned())
}

/// Whether an address is worth reporting: up, global, and neither loopback nor link-local.
fn is_reportable(ip: IpAddr) -> bool {
	match ip {
		IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified(),
		IpAddr::V6(v6) => {
			// `is_unicast_link_local` is not stable, so match fe80::/10 directly.
			let link_local = (v6.segments()[0] & 0xffc0) == 0xfe80;
			!v6.is_loopback() && !link_local && !v6.is_unspecified()
		}
	}
}

/// Every address worth reporting, each with the interface it belongs to. Empty where nothing is up,
/// which is a legitimate answer rather than a failure.
pub fn addresses() -> Vec<Address> {
	let Ok(interfaces) = if_addrs::get_if_addrs() else {
		return Vec::new();
	};
	let mut addresses: Vec<Address> = interfaces
		.into_iter()
		.filter(|interface| is_reportable(interface.addr.ip()))
		.map(|interface| {
			let ip = interface.addr.ip();
			Address {
				address: ip.to_string(),
				interface: interface.name,
				family: if ip.is_ipv4() {
					AddressFamily::Ipv4
				} else {
					AddressFamily::Ipv6
				},
			}
		})
		.collect();
	// A stable order, so a client comparing two reports sees only real changes.
	addresses.sort_by(|a, b| (&a.interface, &a.address).cmp(&(&b.interface, &b.address)));
	addresses
}

/// The device's identity message as it stands right now.
pub fn identity_message() -> DeviceMessage {
	DeviceMessage::Identity {
		hostname: hostname(),
		addresses: addresses(),
	}
}

#[cfg(test)]
mod tests {
	use std::net::{Ipv4Addr, Ipv6Addr};

	use super::*;

	#[test]
	fn loopback_and_link_local_are_not_reported() {
		assert!(!is_reportable(Ipv4Addr::LOCALHOST.into()));
		assert!(!is_reportable(Ipv6Addr::LOCALHOST.into()));
		assert!(!is_reportable(Ipv4Addr::new(169, 254, 3, 4).into()));
		assert!(!is_reportable(
			"fe80::1".parse::<Ipv6Addr>().unwrap().into()
		));
		assert!(!is_reportable(Ipv4Addr::UNSPECIFIED.into()));
	}

	#[test]
	fn routable_and_private_addresses_are_reported() {
		// A private address is what an operator most often needs, so it is reported like any other.
		assert!(is_reportable(Ipv4Addr::new(192, 168, 1, 10).into()));
		assert!(is_reportable(Ipv4Addr::new(203, 0, 113, 5).into()));
		assert!(is_reportable(
			"2001:db8::1".parse::<Ipv6Addr>().unwrap().into()
		));
	}

	#[test]
	fn every_reported_address_carries_its_interface() {
		for address in addresses() {
			assert!(!address.interface.is_empty());
			assert!(address.address.parse::<IpAddr>().is_ok());
		}
	}

	#[test]
	fn the_hostname_is_read_from_the_kernel() {
		let name = hostname();
		assert!(!name.is_empty());
		assert!(!name.contains('\n'));
	}
}
