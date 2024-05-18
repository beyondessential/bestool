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

use std::{sync::Arc, time::Duration};

use bluer::{
	gatt::local::{
		characteristic_control, service_control, Application, ApplicationHandle, Characteristic,
		CharacteristicControl, CharacteristicNotify, CharacteristicNotifyMethod,
		CharacteristicRead, CharacteristicWrite, CharacteristicWriteMethod, Service,
		ServiceControl,
	},
	Adapter, Result, Uuid,
};
use error::Error;
use status::Status;
use tokio::sync::{
	broadcast::{channel as broadcast_channel, Sender},
	RwLock,
};

mod error;
mod rpc;
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
pub struct State<T> {
	inner: Arc<RwLock<InnerState>>,
	status_change_notifier: Sender<()>,
	error_change_notifier: Sender<()>,
	timeout: Option<Duration>,
	handler: T,
}

impl<T> State<T>
where
	T: WifiConfigurator,
{
	async fn status(&self) -> Status {
		self.inner.read().await.status
	}

	async fn last_error(&self) -> Option<Error> {
		self.inner.read().await.last_error
	}

	async fn set_status(&self, new: Status) {
		self.inner.write().await.status = new;
	}

	async fn set_last_error(&self, new: Option<Error>) {
		self.inner.write().await.last_error = new;
	}

	async fn modify_status(&self, status: Status) {
		self.set_status(status).await;
		self.status_change_notifier.send(()).ok();
		// TODO: pro-actively write to the client???
	}

	pub async fn set_error(&self, error: Error) {
		self.set_last_error(Some(error)).await;
		self.error_change_notifier.send(()).ok();
		// TODO: pro-actively write to the client???
	}

	pub async fn clear_error(&self) {
		self.set_last_error(None).await;
		self.error_change_notifier.send(()).ok();
		// TODO: pro-actively write to the client???
	}

	pub async fn set_authorized(&self) {
		if self.status().await == Status::AuthorizationRequired {
			self.modify_status(Status::Authorized).await;
		}
	}

	pub async fn provision(&mut self) {
		if self.status().await != Status::Authorized {
			self.set_error(Error::NotAuthorized).await;
			return;
		}

		self.clear_error().await;

		self.modify_status(Status::Provisioning).await;

		if let Err(err) = self.handler.provision().await {
			self.set_error(err).await;
			self.modify_status(Status::Authorized).await;
			return;
		}

		self.modify_status(Status::Provisioned).await;
	}

	#[tracing::instrument(level = "trace", skip(self))]
	pub async fn handle_raw_rpc(&self, value: Vec<u8>) {
		if let Err(error) = Self::inner_handle_raw_rpc(&self, value).await {
			self.set_error(error).await;
		} else {
			self.clear_error().await;
		}
	}

	async fn inner_handle_raw_rpc(&self, value: Vec<u8>) -> std::result::Result<(), Error> {
		let rpc = rpc::Rpc::parse(&value).map_err(|err| {
			tracing::error!("Failed to parse RPC: {}", err);
			Error::InvalidRPC
		})?;

		todo!()
	}

	pub fn set_timeout(&mut self, timeout: Duration) {
		if T::can_authorize() {
			self.timeout = Some(timeout);
		}
	}
}

#[derive(Debug)]
pub struct ImprovWifi<T> {
	state: State<T>,
	app: ApplicationHandle,
	service: ServiceControl,
	capabilities: CharacteristicControl,
	current_state: CharacteristicControl,
	error_state: CharacteristicControl,
	rpc_command: CharacteristicControl,
	rpc_result: CharacteristicControl,
}

pub trait WifiConfigurator: Clone + Send + Sync + 'static {
	fn can_authorize() -> bool;
	fn can_identify() -> bool;
	async fn provision(&mut self) -> std::result::Result<(), Error>;
}

impl<T> ImprovWifi<T>
where
	T: WifiConfigurator,
{
	pub fn set_timeout(&mut self, timeout: Duration) {
		self.state.set_timeout(timeout);
	}

	pub async fn install(adapter: &Adapter, handler: T) -> Result<Self> {
		let initial_status = if T::can_authorize() {
			Status::AuthorizationRequired
		} else {
			Status::Authorized
		};

		let (status_change_notifier, _) = broadcast_channel(2);
		let (error_change_notifier, _) = broadcast_channel(2);
		let state = State {
			inner: Arc::new(RwLock::new(InnerState {
				status: initial_status,
				last_error: None,
			})),
			status_change_notifier: status_change_notifier.clone(),
			error_change_notifier: error_change_notifier.clone(),
			timeout: if initial_status == Status::AuthorizationRequired {
				Some(Duration::from_secs(60))
			} else {
				None
			},
			handler,
		};

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
									Box::pin(
										async move { Ok(vec![state.status().await.as_byte()]) },
									)
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
													.notify(vec![state.status().await.as_byte()])
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
										Ok(vec![state
											.last_error()
											.await
											.map_or(0x00, |s| s.as_byte())])
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
														.await
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
						write: Some(CharacteristicWrite {
							write: true,
							write_without_response: true,
							method: CharacteristicWriteMethod::Fun(Box::new({
								let state = state.clone();
								move |value, _req| {
									let state = state.clone();
									Box::pin(async move {
										// not sure if the bluer interface here will stitch writes together, let's ignore that for now
										state.handle_raw_rpc(value).await;
										Ok(())
									})
								}
							})),
							..Default::default()
						}),
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
			state,
			app: adapter.serve_gatt_application(app).await?,
			service,
			capabilities: capabilities_control,
			current_state: current_control,
			error_state: error_control,
			rpc_command: rpc_command_control,
			rpc_result: rpc_result_control,
		})
	}
}
