use std::sync::Arc;

use bluer::{
	Adapter,
	gatt::local::{
		Application, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
		CharacteristicRead, CharacteristicWrite, CharacteristicWriteMethod, Service,
	},
};
use tokio::sync::{Mutex, broadcast, watch};

use crate::{
	CHARACTERISTIC_UUID_CAPABILITIES, CHARACTERISTIC_UUID_CURRENT_STATE,
	CHARACTERISTIC_UUID_ERROR_STATE, CHARACTERISTIC_UUID_RPC_COMMAND,
	CHARACTERISTIC_UUID_RPC_RESULT, SERVICE_UUID, Status, WifiConfigurator,
	rpc::Reassembler,
	service::{AuthorizeMode, ImprovWifi, ImprovWifiConfig, InnerState, State},
};

impl<T> ImprovWifi<T>
where
	T: WifiConfigurator,
{
	/// Register the Improv-Wi-Fi GATT application on `adapter` and return a handle.
	///
	/// Call [`ImprovWifi::run`] afterwards to drive advertising, the authorization timeout, and
	/// the shutdown-on-`Provisioned` behaviour.
	pub async fn install(
		adapter: &Adapter,
		configurator: T,
		config: ImprovWifiConfig,
	) -> bluer::Result<Self> {
		let capabilities = configurator.capabilities();
		let auth_required = matches!(config.authorize, AuthorizeMode::Required);
		let initial_status = if auth_required {
			Status::AuthorizationRequired
		} else {
			Status::Authorized
		};

		let (status_tx, _) = broadcast::channel(8);
		let (error_tx, _) = broadcast::channel(8);
		let (rpc_result_tx, _) = broadcast::channel(8);
		let (auth_reset_tx, _) = watch::channel(());
		let (provisioned_tx, provisioned_rx) = watch::channel(false);

		let state = Arc::new(State {
			inner: Mutex::new(InnerState {
				status: initial_status,
				last_error: 0,
				rpc_result: Vec::new(),
			}),
			capabilities,
			configurator,
			reassembler: Mutex::new(Reassembler::new()),
			status_tx: status_tx.clone(),
			error_tx: error_tx.clone(),
			rpc_result_tx: rpc_result_tx.clone(),
			auth_reset_tx,
			provisioned_tx,
			auth_required,
		});

		let app = build_application(state.clone());
		let _app_handle = adapter.serve_gatt_application(app).await?;

		let status_change_for_adv = status_tx.subscribe();

		Ok(Self {
			state,
			adapter: adapter.clone(),
			provisioned_rx,
			status_change_for_adv,
			local_name: config.local_name,
			auth_timeout: config.auth_timeout,
			_app_handle,
		})
	}
}

fn build_application<T>(state: Arc<State<T>>) -> Application
where
	T: WifiConfigurator,
{
	Application {
		services: vec![Service {
			uuid: SERVICE_UUID,
			primary: true,
			characteristics: vec![
				capabilities_char(state.clone()),
				current_state_char(state.clone()),
				error_state_char(state.clone()),
				rpc_command_char(state.clone()),
				rpc_result_char(state),
			],
			..Default::default()
		}],
		..Default::default()
	}
}

fn capabilities_char<T>(state: Arc<State<T>>) -> Characteristic
where
	T: WifiConfigurator,
{
	let cap_byte = state.capabilities.as_byte();
	Characteristic {
		uuid: CHARACTERISTIC_UUID_CAPABILITIES,
		read: Some(CharacteristicRead {
			read: true,
			fun: Box::new(move |_| {
				let byte = cap_byte;
				Box::pin(async move { Ok(vec![byte]) })
			}),
			..Default::default()
		}),
		..Default::default()
	}
}

fn current_state_char<T>(state: Arc<State<T>>) -> Characteristic
where
	T: WifiConfigurator,
{
	let read_state = state.clone();
	let notify_state = state;
	Characteristic {
		uuid: CHARACTERISTIC_UUID_CURRENT_STATE,
		read: Some(CharacteristicRead {
			read: true,
			fun: Box::new(move |_| {
				let s = read_state.clone();
				Box::pin(async move { Ok(vec![s.current_state_byte().await]) })
			}),
			..Default::default()
		}),
		notify: Some(CharacteristicNotify {
			notify: true,
			method: CharacteristicNotifyMethod::Fun(Box::new(move |notifier| {
				let s = notify_state.clone();
				Box::pin(async move {
					let mut rx = s.status_tx.subscribe();
					tokio::spawn(async move {
						let mut notifier = notifier;
						while let Ok(status) = rx.recv().await {
							if notifier.notify(vec![status.as_byte()]).await.is_err() {
								break;
							}
						}
					});
				})
			})),
			..Default::default()
		}),
		..Default::default()
	}
}

fn error_state_char<T>(state: Arc<State<T>>) -> Characteristic
where
	T: WifiConfigurator,
{
	let read_state = state.clone();
	let notify_state = state;
	Characteristic {
		uuid: CHARACTERISTIC_UUID_ERROR_STATE,
		read: Some(CharacteristicRead {
			read: true,
			fun: Box::new(move |_| {
				let s = read_state.clone();
				Box::pin(async move { Ok(vec![s.error_byte().await]) })
			}),
			..Default::default()
		}),
		notify: Some(CharacteristicNotify {
			notify: true,
			method: CharacteristicNotifyMethod::Fun(Box::new(move |notifier| {
				let s = notify_state.clone();
				Box::pin(async move {
					let mut rx = s.error_tx.subscribe();
					tokio::spawn(async move {
						let mut notifier = notifier;
						while let Ok(byte) = rx.recv().await {
							if notifier.notify(vec![byte]).await.is_err() {
								break;
							}
						}
					});
				})
			})),
			..Default::default()
		}),
		..Default::default()
	}
}

fn rpc_command_char<T>(state: Arc<State<T>>) -> Characteristic
where
	T: WifiConfigurator,
{
	Characteristic {
		uuid: CHARACTERISTIC_UUID_RPC_COMMAND,
		write: Some(CharacteristicWrite {
			write: true,
			write_without_response: true,
			method: CharacteristicWriteMethod::Fun(Box::new(move |bytes, _| {
				let s = state.clone();
				Box::pin(async move {
					s.handle_write(bytes).await;
					Ok(())
				})
			})),
			..Default::default()
		}),
		..Default::default()
	}
}

fn rpc_result_char<T>(state: Arc<State<T>>) -> Characteristic
where
	T: WifiConfigurator,
{
	let read_state = state.clone();
	let notify_state = state;
	Characteristic {
		uuid: CHARACTERISTIC_UUID_RPC_RESULT,
		read: Some(CharacteristicRead {
			read: true,
			fun: Box::new(move |_| {
				let s = read_state.clone();
				Box::pin(async move { Ok(s.rpc_result_bytes().await) })
			}),
			..Default::default()
		}),
		notify: Some(CharacteristicNotify {
			notify: true,
			method: CharacteristicNotifyMethod::Fun(Box::new(move |notifier| {
				let s = notify_state.clone();
				Box::pin(async move {
					let mut rx = s.rpc_result_tx.subscribe();
					tokio::spawn(async move {
						let mut notifier = notifier;
						while let Ok(bytes) = rx.recv().await {
							if notifier.notify(bytes).await.is_err() {
								break;
							}
						}
					});
				})
			})),
			..Default::default()
		}),
		..Default::default()
	}
}
