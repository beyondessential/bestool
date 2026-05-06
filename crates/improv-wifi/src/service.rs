use std::{sync::Arc, time::Duration};

use tokio::sync::{Mutex, broadcast, mpsc, watch::Sender as WatchSender};
use tracing::{debug, instrument, warn};
use zbus::{Connection, zvariant::OwnedObjectPath};

use crate::{
	Capabilities, Error, Status, WifiConfigurator,
	bluez::{self, AppHandles},
	rpc::{Command, Reassembler, Yielded, encode_response},
};

/// Whether the device requires explicit user authorisation before accepting credentials.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizeMode {
	/// Start in [`Status::AuthorizationRequired`]; caller must signal authorisation (button press,
	/// etc.) via [`ImprovWifi::authorize`].
	Required,

	/// Start already in [`Status::Authorized`]. The authorisation timeout is not enforced.
	#[default]
	NotRequired,
}

/// Top-level configuration for the Improv-Wi-Fi service.
#[derive(Debug, Default, Clone)]
pub struct ImprovWifiConfig {
	/// How the device gates access to the credential-write commands.
	pub authorize: AuthorizeMode,

	/// How long the device stays in [`Status::Authorized`] before reverting (only meaningful for
	/// [`AuthorizeMode::Required`]). `None` (the default) disables the timeout — the device stays
	/// authorised until provisioned or shut down.
	pub auth_timeout: Option<Duration>,

	/// Local name advertised over BLE. Defaults to the configurator's device name.
	pub local_name: Option<String>,
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

	pub(crate) async fn set_status(&self, new: Status) {
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

/// Improv-Wi-Fi service handle. Construct via [`ImprovWifi::install`], then call
/// [`ImprovWifi::run`] to drive advertising, the authorisation timeout, and the
/// shutdown-on-`Provisioned` behaviour.
pub struct ImprovWifi<T: WifiConfigurator + 'static> {
	state: Arc<State<T>>,
	handles: AppHandles<T>,
}

impl<T> ImprovWifi<T>
where
	T: WifiConfigurator + 'static,
{
	/// Register the Improv-Wi-Fi GATT application + advertisement on the given BlueZ adapter.
	///
	/// `connection` should be a system-bus connection. `adapter_path` is the BlueZ adapter
	/// object path (typically `/org/bluez/hciN`); use [`find_adapter`] to discover one.
	pub async fn install(
		connection: Connection,
		adapter_path: OwnedObjectPath,
		configurator: T,
		config: ImprovWifiConfig,
	) -> Result<Self, Error> {
		let handles = bluez::install(connection, adapter_path, configurator, config).await?;
		let state = handles.state.clone();
		Ok(Self { state, handles })
	}

	/// Signal that the user has authorised the device (e.g. by pressing a button).
	pub async fn authorize(&self) {
		self.state.set_status(Status::Authorized).await;
	}

	/// Get a cloneable handle that can signal authorisation from another task.
	///
	/// The handle stays valid for the lifetime of the channel: triggers fired after
	/// [`Self::run`] returns (i.e. after the service has shut down) are silently dropped.
	pub fn auth_handle(&self) -> AuthHandle {
		AuthHandle {
			tx: self.handles.auth_tx.clone(),
		}
	}

	/// Drive the service until provisioning succeeds, then tear down BLE.
	pub async fn run(self) -> Result<(), Error> {
		bluez::run(self.handles).await
	}
}

/// Cloneable handle for triggering authorisation from another task.
///
/// Obtain via [`ImprovWifi::auth_handle`]. Each call to [`Self::authorize`] drives the
/// service into [`Status::Authorized`] (subject to the existing auth-timeout behaviour).
#[derive(Clone, Debug)]
pub struct AuthHandle {
	tx: mpsc::UnboundedSender<()>,
}

impl AuthHandle {
	/// Signal authorisation. No-op if the service has already shut down.
	pub fn authorize(&self) {
		let _ = self.tx.send(());
	}
}

/// Resolve a BlueZ adapter object path. Pass `None` for the first adapter found.
pub async fn find_adapter(
	connection: &Connection,
	name: Option<&str>,
) -> Result<OwnedObjectPath, Error> {
	bluez::find_adapter(connection, name).await
}

/// Power on the BlueZ adapter at `adapter_path` (sets the `Powered` property to `true`).
pub async fn power_on_adapter(
	connection: &Connection,
	adapter_path: &OwnedObjectPath,
) -> Result<(), Error> {
	bluez::power_on_adapter(connection, adapter_path).await
}
