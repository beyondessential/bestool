use std::sync::{Arc, RwLock};

use bluer::{
	gatt::local::{
		characteristic_control, Characteristic, CharacteristicControl, CharacteristicRead,
	},
	Uuid,
};

const UUID: Uuid = Uuid::from_u128(0x00467768_6228_2272_4663_277478268001);

pub struct ErrorState {
	pub control: CharacteristicControl,
	pub state: Arc<RwLock<Option<Error>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Error {
	/// RPC packet was malformed/invalid.
	InvalidRPC = 0x01,

	/// The command sent is unknown.
	UnknownRPC = 0x02,

	/// The credentials have been received and an attempt to connect to the network has failed.
	UnableToConnect = 0x03,

	/// Credentials were sent via RPC but the Improv service is not authorized.
	NotAuthorized = 0x04,

	/// Unknown error.
	Unknown = 0xFF,
}

impl Error {
	pub fn as_byte(&self) -> u8 {
		*self as _
	}
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
