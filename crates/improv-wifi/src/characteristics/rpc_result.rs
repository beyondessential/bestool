use bluer::{
	gatt::local::{
		characteristic_control, Characteristic, CharacteristicControl, CharacteristicNotify,
		CharacteristicNotifyMethod, CharacteristicWrite, CharacteristicWriteMethod,
	},
	Uuid,
};

const UUID: Uuid = Uuid::from_u128(0x00467768_6228_2272_4663_277478268004);

pub fn install() -> (CharacteristicControl, Characteristic) {
	let (control, control_handle) = characteristic_control();

	(
		control,
		Characteristic {
			uuid: UUID,
			write: Some(CharacteristicWrite {
				write: true,
				write_without_response: true,
				method: CharacteristicWriteMethod::Io,
				..Default::default()
			}),
			notify: Some(CharacteristicNotify {
				notify: true,
				method: CharacteristicNotifyMethod::Io,
				..Default::default()
			}),
			control_handle,
			..Default::default()
		},
	)
}
