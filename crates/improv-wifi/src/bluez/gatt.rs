//! Server-side `org.bluez.GattService1` and `GattCharacteristic1` interfaces.

use std::{collections::HashMap, sync::Arc};

use zbus::{
	interface,
	zvariant::{ObjectPath, OwnedObjectPath, Value},
};

use crate::{WifiConfigurator, service::State};

#[derive(Debug, Clone)]
pub(crate) struct Service {
	pub(crate) uuid: String,
	pub(crate) primary: bool,
}

#[interface(name = "org.bluez.GattService1")]
impl Service {
	#[zbus(property, name = "UUID")]
	fn uuid(&self) -> String {
		self.uuid.clone()
	}

	#[zbus(property)]
	fn primary(&self) -> bool {
		self.primary
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CharKind {
	Capabilities,
	CurrentState,
	ErrorState,
	RpcCommand,
	RpcResult,
}

pub(crate) struct Characteristic<T: WifiConfigurator + 'static> {
	pub(crate) uuid: String,
	pub(crate) service_path: OwnedObjectPath,
	pub(crate) flags: Vec<String>,
	pub(crate) value: Vec<u8>,
	pub(crate) notifying: bool,
	pub(crate) kind: CharKind,
	pub(crate) state: Arc<State<T>>,
}

#[interface(name = "org.bluez.GattCharacteristic1")]
impl<T: WifiConfigurator + 'static> Characteristic<T> {
	#[zbus(property, name = "UUID")]
	fn uuid(&self) -> String {
		self.uuid.clone()
	}

	#[zbus(property)]
	fn service(&self) -> ObjectPath<'_> {
		self.service_path.as_ref()
	}

	#[zbus(property)]
	fn flags(&self) -> Vec<String> {
		self.flags.clone()
	}

	#[zbus(property)]
	fn value(&self) -> Vec<u8> {
		self.value.clone()
	}

	#[zbus(property)]
	fn notifying(&self) -> bool {
		self.notifying
	}

	async fn read_value(&self, _options: HashMap<String, Value<'_>>) -> zbus::fdo::Result<Vec<u8>> {
		let bytes = match self.kind {
			CharKind::Capabilities => vec![self.state.capabilities.as_byte()],
			CharKind::CurrentState => vec![self.state.current_state_byte().await],
			CharKind::ErrorState => vec![self.state.error_byte().await],
			CharKind::RpcCommand => Vec::new(),
			CharKind::RpcResult => self.state.rpc_result_bytes().await,
		};
		Ok(bytes)
	}

	async fn write_value(
		&self,
		value: Vec<u8>,
		_options: HashMap<String, Value<'_>>,
	) -> zbus::fdo::Result<()> {
		if matches!(self.kind, CharKind::RpcCommand) {
			self.state.handle_write(value).await;
		}
		Ok(())
	}

	async fn start_notify(&mut self) -> zbus::fdo::Result<()> {
		self.notifying = true;
		Ok(())
	}

	async fn stop_notify(&mut self) -> zbus::fdo::Result<()> {
		self.notifying = false;
		Ok(())
	}
}
