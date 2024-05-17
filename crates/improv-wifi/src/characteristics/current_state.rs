use std::sync::{Arc, RwLock};

use bluer::{
	gatt::local::{
		characteristic_control, Characteristic, CharacteristicControl, CharacteristicRead,
	},
	Uuid,
};

const UUID: Uuid = Uuid::from_u128(0x00467768_6228_2272_4663_277478268001);

#[derive(Debug)]
pub struct CurrentState {
	pub control: CharacteristicControl,
	pub state: Arc<RwLock<State>>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum State {
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

impl State {
	pub fn as_byte(&self) -> u8 {
		*self as _
	}
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
