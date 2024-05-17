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

use std::{
	sync::{Arc, RwLock},
	time::Duration,
};

use bluer::{
	gatt::local::{
		characteristic_control, service_control, Application, ApplicationHandle, Characteristic,
		CharacteristicControl, CharacteristicNotify, CharacteristicNotifyMethod,
		CharacteristicRead, Service, ServiceControl,
	},
	Adapter, Result, Uuid,
};
use error::Error;
use status::Status;
use tokio::sync::broadcast::{channel as broadcast_channel, Sender};

mod error;
mod status;

const SERVICE_UUID: Uuid = Uuid::from_u128(0x00467768_6228_2272_4663_277478268000);
const CHARACTERISTIC_UUID_CAPABILITIES: Uuid =
	Uuid::from_u128(0x00467768_6228_2272_4663_277478268005);
const CHARACTERISTIC_UUID_CURRENT_STATE: Uuid =
	Uuid::from_u128(0x00467768_6228_2272_4663_277478268001);
const CHARACTERISTIC_UUID_ERROR_STATE: Uuid =
	Uuid::from_u128(0x00467768_6228_2272_4663_277478268002);
const CHARACTERISTIC_UUID_RPC_COMMAND: Uuid =
	Uuid::from_u128(0x00467768_6228_2272_4663_277478268003);
const CHARACTERISTIC_UUID_RPC_RESULT: Uuid =
	Uuid::from_u128(0x00467768_6228_2272_4663_277478268004);

#[derive(Debug)]
pub struct InnerState {
	pub(crate) status: Status,
	pub(crate) last_error: Option<Error>,
}

impl InnerState {
	pub fn status(&self) -> Status {
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

	pub fn status(&self) -> Status {
		self.0.read().unwrap().status
	}

	pub fn last_error(&self) -> Option<Error> {
		self.0.read().unwrap().last_error
	}

	pub fn set_status(&self, new: Status) {
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
	status_change_notifier: Sender<()>,
	error_change_notifier: Sender<()>,
	app: ApplicationHandle,
	service: ServiceControl,
	capabilities: CharacteristicControl,
	current_state: CharacteristicControl,
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
	fn modify_status(&mut self, status: Status) {
		self.state.set_status(status);
		self.status_change_notifier.send(()).ok();
		// TODO: pro-actively write to the client???
	}

	pub fn set_error(&mut self, error: Error) {
		self.state.set_last_error(Some(error));
		self.error_change_notifier.send(()).ok();
		// TODO: pro-actively write to the client???
	}

	pub fn clear_error(&mut self) {
		self.state.set_last_error(None);
		self.error_change_notifier.send(()).ok();
		// TODO: pro-actively write to the client???
	}

	pub fn set_authorized(&mut self) {
		if self.state.status() == Status::AuthorizationRequired {
			self.modify_status(Status::Authorized);
		}
	}

	pub async fn provision(&mut self) {
		if self.state.status() != Status::Authorized {
			self.set_error(Error::NotAuthorized);
			return;
		}

		self.clear_error();

		self.modify_status(Status::Provisioning);

		if let Err(err) = self.handler.provision().await {
			self.set_error(err);
			self.modify_status(Status::Authorized);
			return;
		}

		self.modify_status(Status::Provisioned);
	}

	pub async fn install(adapter: &Adapter, handler: T) -> Result<Self> {
		let initial_status = if T::can_authorize() {
			Status::AuthorizationRequired
		} else {
			Status::Authorized
		};

		let state = State::new(InnerState {
			status: initial_status,
			last_error: None,
		});
		let (status_change_notifier, _) = broadcast_channel(2);
		let (error_change_notifier, _) = broadcast_channel(2);

		let (service, service_handle) = service_control();
		let (capabilities_control, capabilities_handle) = characteristic_control();
		let (current_control, current_handle) = characteristic_control();
		let (error_control, error_handle) = characteristic_control();
		let (rpc_command_control, rpc_command_handle) = characteristic_control();
		let (rpc_result_control, rpc_result_handle) = characteristic_control();

		let app = Application {
			services: vec![Service {
				uuid: SERVICE_UUID,
				primary: true,
				characteristics: vec![
					Characteristic {
						uuid: CHARACTERISTIC_UUID_CAPABILITIES,
						read: Some(CharacteristicRead {
							read: true,
							fun: Box::new(move |_| {
								Box::pin(async move {
									let byte = if T::can_identify() { 1 } else { 0 };
									Ok(vec![byte])
								})
							}),
							..Default::default()
						}),
						control_handle: capabilities_handle,
						..Default::default()
					},
					Characteristic {
						uuid: CHARACTERISTIC_UUID_CURRENT_STATE,
						read: Some(CharacteristicRead {
							read: true,
							fun: Box::new({
								let state = state.clone();
								move |_| {
									let state = state.clone();
									Box::pin(async move { Ok(vec![state.status().as_byte()]) })
								}
							}),
							..Default::default()
						}),
						notify: Some(CharacteristicNotify {
							notify: true,
							method: CharacteristicNotifyMethod::Fun(Box::new({
								let state = state.clone();
								let status_change_notifier = status_change_notifier.clone();
								move |mut notifier| {
									let state = state.clone();
									let mut status_change_receiver =
										status_change_notifier.subscribe();
									Box::pin(async move {
										tokio::spawn(async move {
											while let Ok(()) = status_change_receiver.recv().await {
												notifier
													.notify(vec![state.status().as_byte()])
													.await
													.ok();
											}
										});
									})
								}
							})),
							..Default::default()
						}),
						control_handle: current_handle,
						..Default::default()
					},
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
								}
							}),
							..Default::default()
						}),
						notify: Some(CharacteristicNotify {
							notify: true,
							method: CharacteristicNotifyMethod::Fun(Box::new({
								let state = state.clone();
								let error_change_notifier = error_change_notifier.clone();
								move |mut notifier| {
									let state = state.clone();
									let mut error_change_receiver =
										error_change_notifier.subscribe();
									Box::pin(async move {
										tokio::spawn(async move {
											while let Ok(()) = error_change_receiver.recv().await {
												notifier
													.notify(vec![state
														.last_error()
														.map_or(0x00, |s| s.as_byte())])
													.await
													.ok();
											}
										});
									})
								}
							})),
							..Default::default()
						}),
						control_handle: error_handle,
						..Default::default()
					},
					Characteristic {
						uuid: CHARACTERISTIC_UUID_RPC_COMMAND,
						control_handle: rpc_command_handle,
						..Default::default()
					},
					Characteristic {
						uuid: CHARACTERISTIC_UUID_RPC_RESULT,
						control_handle: rpc_result_handle,
						..Default::default()
					},
				],
				control_handle: service_handle,
				..Default::default()
			}],
			..Default::default()
		};

		Ok(ImprovWifi {
			timeout: if initial_status == Status::AuthorizationRequired {
				Some(Duration::from_secs(60))
			} else {
				None
			},
			state,
			handler,
			status_change_notifier,
			error_change_notifier,
			app: adapter.serve_gatt_application(app).await?,
			service,
			capabilities: capabilities_control,
			current_state: current_control,
			error_state: error_control,
			rpc_command: rpc_command_control,
			rpc_result: rpc_result_control,
		})
	}

	pub fn set_timeout(&mut self, timeout: Duration) {
		if T::can_authorize() {
			self.timeout = Some(timeout);
		}
	}
}
