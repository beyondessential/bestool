use std::sync::{Arc, RwLock};

use bluer::{
	gatt::local::{
		characteristic_control, Characteristic, CharacteristicControl, CharacteristicRead,
	},
	Uuid,
};

const UUID: Uuid = Uuid::from_u128(0x00467768_6228_2272_4663_277478268001);

#[derive(Debug)]
pub struct ErrorState {
	pub control: CharacteristicControl,
	pub state: Arc<RwLock<Option<Error>>>,
}

impl ErrorState {
	pub fn install() -> (Self, Characteristic) {
		let (control, control_handle) = characteristic_control();
		let state = Arc::new(RwLock::new(None));

		(
			ErrorState {
				control,
				state: state.clone(),
			},
			Characteristic {
				uuid: UUID,
				read: Some(CharacteristicRead {
					read: true,
					fun: Box::new(move |_| {
						let state = state.clone();
						Box::pin(async move {
							Ok(vec![state.read().unwrap().map_or(0x00, |s| s.as_byte())])
						})
					}),
					..Default::default()
				}),
				control_handle,
				..Default::default()
			},
		)
	}
}
