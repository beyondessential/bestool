/// Improv Wi-Fi service state, as transmitted on the Current State characteristic.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Status {
	/// Awaiting authorization via physical interaction with the device.
	#[default]
	AuthorizationRequired = 0x01,

	/// Ready to accept credentials.
	Authorized = 0x02,

	/// Credentials received; attempting to connect.
	Provisioning = 0x03,

	/// Connection successful.
	Provisioned = 0x04,
}

impl Status {
	pub fn as_byte(self) -> u8 {
		self as u8
	}
}

/// Capabilities bitfield, as transmitted on the Capabilities characteristic.
///
/// All four bits are independent. The Improv-Wi-Fi spec assigns:
/// - bit 0: identify command supported
/// - bit 1: device-info command supported
/// - bit 2: scan-Wi-Fi command supported
/// - bit 3: hostname command supported
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Capabilities {
	pub identify: bool,
	pub device_info: bool,
	pub scan: bool,
	pub hostname: bool,
}

impl Capabilities {
	pub const fn as_byte(self) -> u8 {
		(self.identify as u8)
			| ((self.device_info as u8) << 1)
			| ((self.scan as u8) << 2)
			| ((self.hostname as u8) << 3)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn status_byte_values() {
		assert_eq!(Status::AuthorizationRequired.as_byte(), 0x01);
		assert_eq!(Status::Authorized.as_byte(), 0x02);
		assert_eq!(Status::Provisioning.as_byte(), 0x03);
		assert_eq!(Status::Provisioned.as_byte(), 0x04);
		assert_eq!(Status::default(), Status::AuthorizationRequired);
	}

	#[test]
	fn capabilities_byte_packing() {
		assert_eq!(Capabilities::default().as_byte(), 0b0000);
		assert_eq!(
			Capabilities {
				identify: true,
				..Default::default()
			}
			.as_byte(),
			0b0001
		);
		assert_eq!(
			Capabilities {
				device_info: true,
				..Default::default()
			}
			.as_byte(),
			0b0010
		);
		assert_eq!(
			Capabilities {
				scan: true,
				..Default::default()
			}
			.as_byte(),
			0b0100
		);
		assert_eq!(
			Capabilities {
				hostname: true,
				..Default::default()
			}
			.as_byte(),
			0b1000
		);
		assert_eq!(
			Capabilities {
				identify: true,
				device_info: true,
				scan: true,
				hostname: true,
			}
			.as_byte(),
			0b1111
		);
	}
}
