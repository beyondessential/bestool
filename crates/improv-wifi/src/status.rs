#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Status {
	/// Awaiting authorization via physical interaction.
	#[default]
	AuthorizationRequired = 0x01,

	/// Ready to accept credentials.
	Authorized = 0x02,

	/// Credentials received, attempt to connect.
	Provisioning = 0x03,

	/// Connection successful.
	Provisioned = 0x04,
}

impl Status {
	pub fn as_byte(&self) -> u8 {
		*self as _
	}
}
