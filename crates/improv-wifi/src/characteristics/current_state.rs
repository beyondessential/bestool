use std::sync::{Arc, RwLock};

use bluer::{
	gatt::local::{
		characteristic_control, Characteristic, CharacteristicControl, CharacteristicRead,
	},
	Uuid,
};

#[derive(Debug)]
pub struct CurrentState {
	pub control: CharacteristicControl,
	pub state: Arc<RwLock<State>>,
}

impl CurrentState {
	pub fn install(state: State) -> (Self, Characteristic) {
		let (control, control_handle) = characteristic_control();
		let state = Arc::new(RwLock::new(state));

		(
			CurrentState {
				control,
				state: state.clone(),
			},
			Characteristic {
				uuid: UUID,
				read: Some(CharacteristicRead {
					read: true,
					fun: Box::new(move |_| {
						let state = state.clone();
						Box::pin(async move { Ok(vec![state.read().unwrap().as_byte()]) })
					}),
					..Default::default()
				}),
				control_handle,
				..Default::default()
			},
		)
	}
}
