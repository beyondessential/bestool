//! Characteristic: Capabilities of the Improv device
//!
//! | Bit (Lsb) | Description                        |
//! |:----------|:-----------------------------------|
//! | `0`       | 1: supports the `identify` command |

use bluer::{
	gatt::local::{
		characteristic_control, Characteristic, CharacteristicControl, CharacteristicRead,
	},
	Uuid,
};

const UUID: Uuid = Uuid::from_u128(0x00467768_6228_2272_4663_277478268005);

#[derive(Debug)]
pub struct Capabilities {
	pub control: CharacteristicControl,
	pub can_identify: bool,
}

impl Capabilities {
	pub fn as_byte(&self) -> u8 {
		(self.can_identify as u8) << 0
	}

	pub fn install(can_identify: bool) -> (Self, Characteristic) {
		let (control, control_handle) = characteristic_control();
		let this = Self {
			control,
			can_identify,
		};
		let byte = this.as_byte();

		(
			this,
			Characteristic {
				uuid: UUID,
				read: Some(CharacteristicRead {
					read: true,
					fun: Box::new(move |_| {
						Box::pin(async move { Ok(vec![byte]) })
					}),
					..Default::default()
				}),
				control_handle,
				..Default::default()
			},
		)
	}
}
