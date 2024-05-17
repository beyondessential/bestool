#![cfg(target_os = "linux")]
//! An implementation of [improv-wifi] for Linux.
//!
//! This crate provides an implementation of the Improv Wi-Fi configuration protocol, as a
//! peripheral, via BlueZ's D-Bus API. It is intended to be used in conjunction with the
//! [improv-wifi] tooling for Web and Android, to allow for easy connection of an embedded Linux
//! device to a Wi-Fi network without the need for a display or other input peripherals.
//!
//! As there are many different network systems available for Linux, this crate supplies a trait to
//! allow any network configuration framework to be supported.
//!
//! # Examples
//!
//! # Features
//!
//! - `miette`: Implements `miette::Diagnostic` on the error type.
//! - `networkmanager`: Enables the `NetworkManager` wifi configurator.
//!
//! [improv-wifi]: https://www.improv-wifi.com
//! [NetworkManager]: https://www.networkmanager.dev

use std::{sync::{Arc, RwLock}, time::Duration};

use bluer::{
	gatt::local::{
		characteristic_control, service_control, Application, ApplicationHandle, Characteristic, CharacteristicControl, CharacteristicRead, Service, ServiceControl
	},
	Adapter, Result, Uuid,
};
use characteristics::{
	capabilities::Capabilities,
	current_state::{CurrentState, State as CState},
};
use error::Error;

mod characteristics;
mod error;

const SERVICE_UUID: Uuid = Uuid::from_u128(0x00467768_6228_2272_4663_277478268000);
const CHARACTERISTIC_UUID_ERROR_STATE: Uuid = Uuid::from_u128(0x00467768_6228_2272_4663_277478268001);

#[derive(Debug)]
pub struct InnerState {
	pub(crate) status: CState,
	pub(crate) last_error: Option<Error>,
}

impl InnerState {
	pub fn status(&self) -> CState {
		self.status
	}

	pub fn last_error(&self) -> Option<Error> {
		self.last_error
	}
}

#[derive(Clone, Debug)]
pub struct State(Arc<RwLock<InnerState>>);

impl State {
	pub fn new(state: InnerState) -> Self {
		Self(Arc::new(RwLock::new(state)))
	}

	pub fn status(&self) -> CState {
		self.0.read().unwrap().status
	}

	pub fn last_error(&self) -> Option<Error> {
		self.0.read().unwrap().last_error
	}

	pub fn set_status(&self, new: CState) {
		self.0.write().unwrap().status = new;
	}

	pub fn set_last_error(&self, new: Option<Error>) {
		self.0.write().unwrap().last_error = new;
	}
}

#[derive(Debug)]
pub struct ImprovWifi<T> {
	timeout: Option<Duration>,
	state: State,
	handler: T,
	app: ApplicationHandle,
	service: ServiceControl,
	capabilities: Capabilities,
	current_state: CurrentState,
	error_state: CharacteristicControl,
	rpc_command: CharacteristicControl,
	rpc_result: CharacteristicControl,
}

pub trait WifiConfigurator {
	fn can_authorize() -> bool;
	fn can_identify() -> bool;
	async fn provision(&mut self) -> std::result::Result<(), Error>;
}

impl<T: WifiConfigurator> ImprovWifi<T> {
	fn notify_error(&self) {
		let error_byte = self.state.last_error().map_or(0, |e| e.as_byte());
		todo!("write and notify the error to a connected client");
	}

	fn notify_state(&self) {
		let state_byte = self.state.status().as_byte();
		todo!("write and notify the state to a connected client");
	}

	fn modify_state(&mut self, state: CState) {
		self.state.set_status(state);
		self.notify_state();
	}

	pub fn set_error(&mut self, error: Error) {
		self.state.set_last_error(Some(error));
		self.notify_error();
	}

	pub fn clear_error(&mut self) {
		self.state.set_last_error(None);
		self.notify_error();
	}

	pub fn set_authorized(&mut self) {
		if self.state.status() == CState::AuthorizationRequired {
			self.modify_state(CState::Authorized);
		}
	}

	pub async fn provision(&mut self) {
		if self.state.status() != CState::Authorized {
			self.set_error(Error::NotAuthorized);
			return;
		}

		self.clear_error();

		self.modify_state(CState::Provisioning);

		if let Err(err) = self.handler.provision().await {
			self.set_error(err);
			self.modify_state(CState::Authorized);
			return;
		}

		self.modify_state(CState::Provisioned);
	}

	pub async fn install(adapter: &Adapter, handler: T) -> Result<Self> {
		let initial_state = if T::can_authorize() {
			CState::AuthorizationRequired
		} else {
			CState::Authorized
		};

		let state = State::new(InnerState {
			status: initial_state,
			last_error: None,
		});

		let (service, service_handle) = service_control();
		let (capabilities, capabilities_char) = Capabilities::install(T::can_identify());
		let (current_state, current_state_char) = CurrentState::install(initial_state);
		let (error_control, error_handle) = characteristic_control();
		let (rpc_command, rpc_command_char) = characteristics::rpc_command::install();
		let (rpc_result, rpc_result_char) = characteristics::rpc_result::install();

		let app = Application {
			services: vec![Service {
				uuid: SERVICE_UUID,
				primary: true,
				characteristics: vec![
					capabilities_char,
					current_state_char,
					Characteristic {
						uuid: CHARACTERISTIC_UUID_ERROR_STATE,
						read: Some(CharacteristicRead {
							read: true,
							fun: Box::new({
								let state = state.clone();
								move |_| {
								let state = state.clone();
								Box::pin(async move {
									Ok(vec![state.last_error().map_or(0x00, |s| s.as_byte())])
								})
							}}),
							..Default::default()
						}),
						control_handle: error_handle,
						..Default::default()
					},
					rpc_command_char,
					rpc_result_char,
				],
				control_handle: service_handle,
				..Default::default()
			}],
			..Default::default()
		};

		Ok(ImprovWifi {
			timeout: if initial_state == CState::AuthorizationRequired {
				Some(Duration::from_secs(60))
			} else {
				None
			},
			state,
			handler,
			app: adapter.serve_gatt_application(app).await?,
			service,
			capabilities,
			current_state,
			error_state: error_control,
			rpc_command,
			rpc_result,
		})
	}

	pub fn set_timeout(&mut self, timeout: Duration) {
		if T::can_authorize() {
			self.timeout = Some(timeout);
		}
	}
}
