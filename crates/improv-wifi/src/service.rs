use std::{sync::Arc, time::Duration};

use bluer::{
	Adapter,
	adv::{Advertisement, AdvertisementHandle, Type as AdvType},
	gatt::local::ApplicationHandle,
};
use tokio::sync::{
	Mutex, broadcast,
	watch::{Receiver as WatchReceiver, Sender as WatchSender},
};
use tracing::{debug, info, instrument, warn};

use crate::{
	Capabilities, Error, Status, WifiConfigurator,
	rpc::{Command, Reassembler, Yielded, encode_response},
};

/// Whether the device requires explicit user authorization before accepting credentials.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizeMode {
	/// Start in [`Status::AuthorizationRequired`]; caller must signal authorization (button press,
	/// etc.) via [`ImprovWifi::authorize`].
	Required,

	/// Start already in [`Status::Authorized`]. The authorization timeout is not enforced.
	#[default]
	NotRequired,
}

/// Top-level configuration for the Improv-Wi-Fi service.
#[derive(Debug, Clone)]
pub struct ImprovWifiConfig {
	/// How the device gates access to the credential-write commands.
	pub authorize: AuthorizeMode,

	/// How long the device stays in [`Status::Authorized`] before reverting (only meaningful for
	/// [`AuthorizeMode::Required`]). Per spec, ~60 seconds is suggested.
	pub auth_timeout: Duration,

	/// Local name advertised over BLE. Defaults to the configurator's device name.
	pub local_name: Option<String>,
}

impl Default for ImprovWifiConfig {
	fn default() -> Self {
		Self {
			authorize: AuthorizeMode::default(),
			auth_timeout: Duration::from_secs(60),
			local_name: None,
		}
	}
}

#[derive(Debug)]
pub(crate) struct InnerState {
	pub(crate) status: Status,
	pub(crate) last_error: u8, // 0 = no error
	pub(crate) rpc_result: Vec<u8>,
}

/// Shared service state, held behind an `Arc` and mutated from BLE callbacks.
pub(crate) struct State<T> {
	pub(crate) inner: Mutex<InnerState>,
	pub(crate) capabilities: Capabilities,
	pub(crate) configurator: T,
	pub(crate) reassembler: Mutex<Reassembler>,
	pub(crate) status_tx: broadcast::Sender<Status>,
	pub(crate) error_tx: broadcast::Sender<u8>,
	pub(crate) rpc_result_tx: broadcast::Sender<Vec<u8>>,
	pub(crate) auth_reset_tx: WatchSender<()>,
	pub(crate) provisioned_tx: WatchSender<bool>,
	pub(crate) auth_required: bool,
}

impl<T> State<T>
where
	T: WifiConfigurator,
{
	pub(crate) async fn current_state_byte(&self) -> u8 {
		self.inner.lock().await.status.as_byte()
	}

	pub(crate) async fn error_byte(&self) -> u8 {
		self.inner.lock().await.last_error
	}

	pub(crate) async fn rpc_result_bytes(&self) -> Vec<u8> {
		self.inner.lock().await.rpc_result.clone()
	}

	async fn set_status(&self, new: Status) {
		{
			let mut inner = self.inner.lock().await;
			if inner.status == new {
				return;
			}
			inner.status = new;
		}
		let _ = self.status_tx.send(new);
		if new == Status::Authorized {
			let _ = self.auth_reset_tx.send(());
		}
		if new == Status::Provisioned {
			let _ = self.provisioned_tx.send(true);
		}
	}

	async fn set_error(&self, err: Option<Error>) {
		let byte = err.map_or(0, Error::as_byte);
		{
			let mut inner = self.inner.lock().await;
			if inner.last_error == byte {
				return;
			}
			inner.last_error = byte;
		}
		let _ = self.error_tx.send(byte);
	}

	async fn set_rpc_result(&self, bytes: Vec<u8>) {
		{
			let mut inner = self.inner.lock().await;
			inner.rpc_result = bytes.clone();
		}
		let _ = self.rpc_result_tx.send(bytes);
	}

	#[instrument(level = "debug", skip(self, write))]
	pub(crate) async fn handle_write(&self, write: Vec<u8>) {
		let yielded = self.reassembler.lock().await.feed(&write);
		for item in yielded {
			match item {
				Yielded::Command(cmd) => self.dispatch(cmd).await,
				Yielded::Error(parse_err) => {
					warn!(?parse_err, "RPC parse error");
					let mapped = match parse_err {
						crate::rpc::ParseError::UnknownCommand(_) => Error::UnknownRPC,
						_ => Error::InvalidRPC,
					};
					self.set_error(Some(mapped)).await;
				}
			}
		}
	}

	async fn dispatch(&self, cmd: Command) {
		debug!(?cmd, "RPC command");
		match cmd {
			Command::Identify => {
				self.set_error(None).await;
				if let Err(err) = self.configurator.identify().await {
					self.set_error(Some(err)).await;
				}
			}
			Command::DeviceInfo => {
				self.respond(
					0x03,
					self.configurator
						.device_info()
						.await
						.map(|i| i.into_strings()),
				)
				.await
			}
			Command::Scan => {
				let res = self.configurator.scan().await.map(|nets| {
					let mut out = Vec::with_capacity(nets.len() * 3);
					for n in nets {
						out.push(n.ssid);
						out.push(n.rssi.to_string());
						out.push(n.auth);
					}
					out
				});
				self.respond(0x04, res).await;
			}
			Command::GetHostname => {
				let res = self.configurator.get_hostname().await.map(|h| vec![h]);
				self.respond(0x05, res).await;
			}
			Command::SetHostname(name) => {
				if !self.is_authorized().await {
					self.set_error(Some(Error::NotAuthorized)).await;
					return;
				}
				let _ = self.auth_reset_tx.send(());
				match self.configurator.set_hostname(name.clone()).await {
					Ok(()) => {
						self.set_error(None).await;
						self.set_rpc_result(encode_response(0x05, &[name])).await;
					}
					Err(err) => self.set_error(Some(err)).await,
				}
			}
			Command::GetDeviceName => {
				let res = self.configurator.get_device_name().await.map(|n| vec![n]);
				self.respond(0x06, res).await;
			}
			Command::SetDeviceName(name) => {
				if !self.is_authorized().await {
					self.set_error(Some(Error::NotAuthorized)).await;
					return;
				}
				let _ = self.auth_reset_tx.send(());
				match self.configurator.set_device_name(name.clone()).await {
					Ok(()) => {
						self.set_error(None).await;
						self.set_rpc_result(encode_response(0x06, &[name])).await;
					}
					Err(err) => self.set_error(Some(err)).await,
				}
			}
			Command::SendWifiSettings { ssid, password } => {
				if !self.is_authorized().await {
					self.set_error(Some(Error::NotAuthorized)).await;
					return;
				}
				self.set_error(None).await;
				self.set_status(Status::Provisioning).await;
				match self.configurator.provision(ssid, password).await {
					Ok(strings) => {
						self.set_rpc_result(encode_response(0x01, &strings)).await;
						self.set_status(Status::Provisioned).await;
					}
					Err(err) => {
						self.set_error(Some(err)).await;
						self.set_status(Status::Authorized).await;
					}
				}
			}
		}
	}

	async fn respond(&self, command_id: u8, res: Result<Vec<String>, Error>) {
		match res {
			Ok(strings) => {
				self.set_error(None).await;
				self.set_rpc_result(encode_response(command_id, &strings))
					.await;
			}
			Err(err) => self.set_error(Some(err)).await,
		}
	}

	async fn is_authorized(&self) -> bool {
		matches!(self.inner.lock().await.status, Status::Authorized)
	}
}

pub struct ImprovWifi<T> {
	pub(crate) state: Arc<State<T>>,
	pub(crate) adapter: Adapter,
	pub(crate) provisioned_rx: WatchReceiver<bool>,
	pub(crate) status_change_for_adv: broadcast::Receiver<Status>,
	pub(crate) local_name: Option<String>,
	pub(crate) auth_timeout: Duration,
	pub(crate) _app_handle: ApplicationHandle,
}

impl<T> ImprovWifi<T>
where
	T: WifiConfigurator,
{
	/// Signal that the user has authorized the device (e.g. by pressing a button).
	pub async fn authorize(&self) {
		self.state.set_status(Status::Authorized).await;
	}

	pub(crate) fn build_advertisement(&self, status_byte: u8) -> Advertisement {
		let cap_byte = self.state.capabilities.as_byte();
		let service_data = vec![status_byte, cap_byte, 0, 0, 0, 0];
		let mut sd = std::collections::BTreeMap::new();
		sd.insert(crate::ADVERTISEMENT_SERVICE_DATA_UUID, service_data);

		let mut uuids = std::collections::BTreeSet::new();
		uuids.insert(crate::SERVICE_UUID);

		Advertisement {
			advertisement_type: AdvType::Peripheral,
			service_uuids: uuids,
			service_data: sd,
			discoverable: Some(true),
			local_name: self.local_name.clone(),
			..Default::default()
		}
	}

	/// Drive the service until provisioning succeeds, then tear down BLE.
	pub async fn run(mut self) -> bluer::Result<()> {
		let mut provisioned = self.provisioned_rx.clone();
		let mut adv_handle: Option<AdvertisementHandle> = Some(
			self.adapter
				.advertise(self.build_advertisement(self.state.current_state_byte().await))
				.await?,
		);

		// Authorization timeout task: only relevant when authorization is required. We re-arm
		// the timer on each `auth_reset_tx` tick, and on expiry transition Authorized →
		// AuthorizationRequired.
		let auth_required = self.state.auth_required;
		let auth_timeout = self.auth_timeout;
		let timeout_state = self.state.clone();
		let mut auth_reset_rx = self.state.auth_reset_tx.subscribe();
		let mut provisioned_for_timeout = self.provisioned_rx.clone();
		let timeout_task = tokio::spawn(async move {
			if !auth_required {
				return;
			}
			loop {
				let sleep = tokio::time::sleep(auth_timeout);
				tokio::pin!(sleep);
				tokio::select! {
					biased;
					_ = provisioned_for_timeout.changed() => {
						if *provisioned_for_timeout.borrow() {
							return;
						}
					}
					res = auth_reset_rx.changed() => {
						if res.is_err() {
							return;
						}
						continue;
					}
					_ = &mut sleep => {
						if matches!(timeout_state.inner.lock().await.status, Status::Authorized) {
							info!("authorization timed out, reverting to AuthorizationRequired");
							timeout_state.set_status(Status::AuthorizationRequired).await;
						}
					}
				}
			}
		});

		// Wait for either a status change (re-issue advertisement to update service-data byte) or
		// `provisioned` becoming true (tear down).
		loop {
			tokio::select! {
				res = self.status_change_for_adv.recv() => {
					match res {
						Ok(_) => {
							drop(adv_handle.take());
							let new_byte = self.state.current_state_byte().await;
							adv_handle = Some(self.adapter.advertise(self.build_advertisement(new_byte)).await?);
						}
						Err(broadcast::error::RecvError::Lagged(_)) => continue,
						Err(broadcast::error::RecvError::Closed) => break,
					}
				}
				res = provisioned.changed() => {
					if res.is_err() {
						break;
					}
					if *provisioned.borrow() {
						info!("provisioning successful, shutting down Improv service");
						break;
					}
				}
			}
		}

		drop(adv_handle);
		timeout_task.abort();
		Ok(())
	}
}
