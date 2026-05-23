//! Improv-Wi-Fi BLE peripheral lifecycle: connect, register, run, tear down.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::{
	sync::{Mutex, broadcast, mpsc, watch},
	task::JoinHandle,
};
use tracing::{debug, info, warn};
use zbus::{
	Connection,
	zvariant::{ObjectPath, OwnedObjectPath},
};

use crate::{
	ADVERTISEMENT_SERVICE_DATA_UUID, CHARACTERISTIC_UUID_CAPABILITIES,
	CHARACTERISTIC_UUID_CURRENT_STATE, CHARACTERISTIC_UUID_ERROR_STATE,
	CHARACTERISTIC_UUID_RPC_COMMAND, CHARACTERISTIC_UUID_RPC_RESULT, Error, SERVICE_UUID, Status,
	WifiConfigurator,
	bluez::{
		advertisement::Advertisement,
		gatt::{CharKind, Characteristic, Service},
		proxy::{
			Adapter1Proxy, BluezObjectManagerProxy, GattManager1Proxy, LEAdvertisingManager1Proxy,
		},
	},
	rpc::Reassembler,
	service::{AuthorizeMode, ImprovWifiConfig, InnerState, State},
};

const APP_PATH: &str = "/au/bes/improv";
const SERVICE_PATH: &str = "/au/bes/improv/service0";
const ADV_PATH: &str = "/au/bes/improv/adv0";

const ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";

/// Power on the BlueZ adapter at `adapter_path`.
pub(crate) async fn power_on_adapter(
	connection: &Connection,
	adapter_path: &OwnedObjectPath,
) -> Result<(), Error> {
	let adapter = Adapter1Proxy::builder(connection)
		.path(adapter_path.clone())
		.map_err(map)?
		.build()
		.await
		.map_err(map)?;
	adapter.set_powered(true).await.map_err(map)
}

/// Resolve an adapter object path on `org.bluez`. If `name` is `Some("hciN")`, looks for a
/// matching adapter; otherwise returns the first adapter found.
pub(crate) async fn find_adapter(
	connection: &Connection,
	name: Option<&str>,
) -> Result<OwnedObjectPath, Error> {
	let manager = BluezObjectManagerProxy::new(connection)
		.await
		.map_err(map)?;
	let objects = manager.get_managed_objects().await.map_err(map)?;
	let mut found_first: Option<OwnedObjectPath> = None;
	let want_suffix = name.map(|n| format!("/{n}"));
	for (path, ifaces) in objects {
		if !ifaces.contains_key(ADAPTER_INTERFACE) {
			continue;
		}
		if let Some(suffix) = &want_suffix {
			if path.as_str().ends_with(suffix) {
				return Ok(path);
			}
		} else if found_first.is_none() {
			found_first = Some(path);
		}
	}
	if let Some(suffix) = want_suffix {
		warn!(adapter = %&suffix[1..], "no matching BlueZ adapter found");
		return Err(Error::Unknown);
	}
	found_first.ok_or_else(|| {
		warn!("no BlueZ adapters found");
		Error::Unknown
	})
}

/// Handles for everything we registered with BlueZ. Dropping the inner state shuts the GATT
/// callbacks down; explicit unregister calls happen in `run` after the loop exits.
pub(crate) struct AppHandles<T: WifiConfigurator + 'static> {
	pub(crate) connection: Connection,
	pub(crate) adapter_path: OwnedObjectPath,
	pub(crate) state: Arc<State<T>>,
	pub(crate) provisioned_rx: watch::Receiver<bool>,
	pub(crate) status_change_for_adv: broadcast::Receiver<Status>,
	pub(crate) local_name: Option<String>,
	pub(crate) auth_timeout: Option<Duration>,
	pub(crate) notify_tasks: Vec<JoinHandle<()>>,
	pub(crate) auth_tx: mpsc::UnboundedSender<()>,
	pub(crate) auth_rx: mpsc::UnboundedReceiver<()>,
}

/// Build the shared state, register all objects on the object server, and call
/// `RegisterApplication` + `RegisterAdvertisement`.
pub(crate) async fn install<T: WifiConfigurator + 'static>(
	connection: Connection,
	adapter_path: OwnedObjectPath,
	configurator: T,
	config: ImprovWifiConfig,
) -> Result<AppHandles<T>, Error> {
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
	let (auth_tx, auth_rx) = mpsc::unbounded_channel();
	let status_change_for_adv = status_tx.subscribe();

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

	let object_server = connection.object_server();

	// Service object.
	let service = Service {
		uuid: SERVICE_UUID.to_string(),
		primary: true,
	};
	object_server.at(SERVICE_PATH, service).await.map_err(map)?;

	let service_path = OwnedObjectPath::try_from(SERVICE_PATH).map_err(|_| Error::Unknown)?;

	// Characteristic objects.
	let chars: &[(CharKind, &str, uuid::Uuid, &[&str])] = &[
		(
			CharKind::Capabilities,
			"char0",
			CHARACTERISTIC_UUID_CAPABILITIES,
			&["read"],
		),
		(
			CharKind::CurrentState,
			"char1",
			CHARACTERISTIC_UUID_CURRENT_STATE,
			&["read", "notify"],
		),
		(
			CharKind::ErrorState,
			"char2",
			CHARACTERISTIC_UUID_ERROR_STATE,
			&["read", "notify"],
		),
		(
			CharKind::RpcCommand,
			"char3",
			CHARACTERISTIC_UUID_RPC_COMMAND,
			&["write", "write-without-response"],
		),
		(
			CharKind::RpcResult,
			"char4",
			CHARACTERISTIC_UUID_RPC_RESULT,
			&["read", "notify"],
		),
	];

	let mut notify_paths: Vec<(CharKind, String)> = Vec::new();
	for (kind, leaf, uuid, flags) in chars {
		let path = format!("{SERVICE_PATH}/{leaf}");
		let initial_value = match kind {
			CharKind::Capabilities => vec![capabilities.as_byte()],
			CharKind::CurrentState => vec![initial_status.as_byte()],
			CharKind::ErrorState => vec![0],
			_ => Vec::new(),
		};
		let char_obj = Characteristic {
			uuid: uuid.to_string(),
			service_path: service_path.clone(),
			flags: flags.iter().map(|s| (*s).to_string()).collect(),
			value: initial_value,
			notifying: false,
			kind: *kind,
			state: state.clone(),
		};
		object_server
			.at(path.clone(), char_obj)
			.await
			.map_err(map)?;
		if flags.contains(&"notify") {
			notify_paths.push((*kind, path));
		}
	}

	// Initial advertisement.
	let initial_adv = build_advertisement(
		&capabilities,
		initial_status.as_byte(),
		config.local_name.as_deref(),
	);
	object_server.at(ADV_PATH, initial_adv).await.map_err(map)?;

	// Register the application + advertisement with BlueZ on the chosen adapter.
	let gatt_mgr = GattManager1Proxy::builder(&connection)
		.path(adapter_path.clone())
		.map_err(map)?
		.build()
		.await
		.map_err(map)?;
	let app_path = ObjectPath::try_from(APP_PATH).map_err(|_| Error::Unknown)?;
	gatt_mgr
		.register_application(&app_path, HashMap::new())
		.await
		.map_err(|err| {
			warn!(?err, "GattManager1.RegisterApplication failed");
			Error::Unknown
		})?;
	debug!("registered GATT application with BlueZ");

	let adv_mgr = LEAdvertisingManager1Proxy::builder(&connection)
		.path(adapter_path.clone())
		.map_err(map)?
		.build()
		.await
		.map_err(map)?;
	let adv_path = ObjectPath::try_from(ADV_PATH).map_err(|_| Error::Unknown)?;
	adv_mgr
		.register_advertisement(&adv_path, HashMap::new())
		.await
		.map_err(|err| {
			warn!(?err, "LEAdvertisingManager1.RegisterAdvertisement failed");
			// Best-effort cleanup of GATT registration on failure.
			Error::Unknown
		})?;
	debug!("registered LE advertisement with BlueZ");

	// Spawn notify-push tasks: each notify-capable characteristic subscribes to its broadcast
	// channel and emits PropertiesChanged on `Value` whenever a fresh value comes through.
	let mut notify_tasks = Vec::with_capacity(notify_paths.len());
	for (kind, path) in notify_paths {
		let conn = connection.clone();
		let st = state.clone();
		let task = match kind {
			CharKind::CurrentState => {
				let mut rx = status_tx.subscribe();
				tokio::spawn(async move {
					while let Ok(status) = rx.recv().await {
						push_value::<T>(&conn, &path, vec![status.as_byte()]).await;
					}
					let _ = st;
				})
			}
			CharKind::ErrorState => {
				let mut rx = error_tx.subscribe();
				tokio::spawn(async move {
					while let Ok(byte) = rx.recv().await {
						push_value::<T>(&conn, &path, vec![byte]).await;
					}
					let _ = st;
				})
			}
			CharKind::RpcResult => {
				let mut rx = rpc_result_tx.subscribe();
				tokio::spawn(async move {
					while let Ok(bytes) = rx.recv().await {
						push_value::<T>(&conn, &path, bytes).await;
					}
					let _ = st;
				})
			}
			_ => continue,
		};
		notify_tasks.push(task);
	}

	Ok(AppHandles {
		connection,
		adapter_path,
		state,
		provisioned_rx,
		status_change_for_adv,
		local_name: config.local_name,
		auth_timeout: config.auth_timeout,
		notify_tasks,
		auth_tx,
		auth_rx,
	})
}

/// Run the service until provisioning succeeds, then unregister everything.
pub(crate) async fn run<T: WifiConfigurator + 'static>(
	mut handles: AppHandles<T>,
) -> Result<(), Error> {
	let auth_required = handles.state.auth_required;
	let auth_timeout = handles.auth_timeout;
	let timeout_state = handles.state.clone();
	let mut auth_reset_rx = handles.state.auth_reset_tx.subscribe();
	let mut provisioned_for_timeout = handles.provisioned_rx.clone();
	let timeout_task: Option<JoinHandle<()>> = match (auth_required, auth_timeout) {
		(true, Some(auth_timeout)) => Some(tokio::spawn(async move {
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
							info!("authorisation timed out, reverting to AuthorizationRequired");
							timeout_state.set_status(Status::AuthorizationRequired).await;
						}
					}
				}
			}
		})),
		_ => None,
	};

	let mut provisioned = handles.provisioned_rx.clone();

	loop {
		tokio::select! {
			res = handles.status_change_for_adv.recv() => {
				match res {
					Ok(_) => {
						let new_byte = handles.state.current_state_byte().await;
						refresh_advertisement(
							&handles.connection,
							&handles.adapter_path,
							&handles.state.capabilities,
							new_byte,
							handles.local_name.as_deref(),
						).await?;
					}
					Err(broadcast::error::RecvError::Lagged(_)) => continue,
					Err(broadcast::error::RecvError::Closed) => break,
				}
			}
			Some(()) = handles.auth_rx.recv() => {
				debug!("authorisation signal received");
				handles.state.set_status(Status::Authorized).await;
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

	if let Some(task) = timeout_task {
		task.abort();
	}
	for task in handles.notify_tasks.drain(..) {
		task.abort();
	}

	// Best-effort unregister.
	let adv_path = ObjectPath::try_from(ADV_PATH).map_err(|_| Error::Unknown)?;
	let app_path = ObjectPath::try_from(APP_PATH).map_err(|_| Error::Unknown)?;

	match LEAdvertisingManager1Proxy::builder(&handles.connection)
		.path(handles.adapter_path.clone())
	{
		Ok(builder) => match builder.build().await {
			Ok(p) => {
				if let Err(err) = p.unregister_advertisement(&adv_path).await {
					debug!(?err, "UnregisterAdvertisement failed (continuing)");
				}
			}
			Err(err) => debug!(?err, "build LEAdvertisingManager1 proxy for cleanup failed"),
		},
		Err(err) => debug!(?err, "LEAdvertisingManager1 builder path failed"),
	}

	match GattManager1Proxy::builder(&handles.connection).path(handles.adapter_path.clone()) {
		Ok(builder) => match builder.build().await {
			Ok(p) => {
				if let Err(err) = p.unregister_application(&app_path).await {
					debug!(?err, "UnregisterApplication failed (continuing)");
				}
			}
			Err(err) => debug!(?err, "build GattManager1 proxy for cleanup failed"),
		},
		Err(err) => debug!(?err, "GattManager1 builder path failed"),
	}

	let object_server = handles.connection.object_server();
	let _ = object_server.remove::<Advertisement, _>(ADV_PATH).await;
	for leaf in ["char0", "char1", "char2", "char3", "char4"] {
		let _ = object_server
			.remove::<Characteristic<T>, _>(format!("{SERVICE_PATH}/{leaf}"))
			.await;
	}
	let _ = object_server.remove::<Service, _>(SERVICE_PATH).await;

	Ok(())
}

fn build_advertisement(
	capabilities: &crate::Capabilities,
	status_byte: u8,
	local_name: Option<&str>,
) -> Advertisement {
	let cap_byte = capabilities.as_byte();
	let mut service_data = HashMap::new();
	service_data.insert(
		ADVERTISEMENT_SERVICE_DATA_UUID.to_string(),
		vec![status_byte, cap_byte, 0, 0, 0, 0],
	);
	Advertisement {
		advertisement_type: "peripheral".to_owned(),
		service_uuids: vec![SERVICE_UUID.to_string()],
		service_data,
		local_name: local_name.map(str::to_owned),
		discoverable: true,
	}
}

async fn refresh_advertisement(
	connection: &Connection,
	adapter_path: &OwnedObjectPath,
	capabilities: &crate::Capabilities,
	status_byte: u8,
	local_name: Option<&str>,
) -> Result<(), Error> {
	let adv_path = ObjectPath::try_from(ADV_PATH).map_err(|_| Error::Unknown)?;
	let adv_mgr = LEAdvertisingManager1Proxy::builder(connection)
		.path(adapter_path.clone())
		.map_err(map)?
		.build()
		.await
		.map_err(map)?;

	if let Err(err) = adv_mgr.unregister_advertisement(&adv_path).await {
		debug!(?err, "UnregisterAdvertisement before refresh failed");
	}

	let object_server = connection.object_server();
	let _ = object_server.remove::<Advertisement, _>(ADV_PATH).await;
	let new_adv = build_advertisement(capabilities, status_byte, local_name);
	object_server.at(ADV_PATH, new_adv).await.map_err(map)?;

	adv_mgr
		.register_advertisement(&adv_path, HashMap::new())
		.await
		.map_err(|err| {
			warn!(?err, "RegisterAdvertisement during refresh failed");
			Error::Unknown
		})?;
	Ok(())
}

async fn push_value<T: WifiConfigurator + 'static>(
	connection: &Connection,
	path: &str,
	new_value: Vec<u8>,
) {
	match connection
		.object_server()
		.interface::<_, Characteristic<T>>(path)
		.await
	{
		Ok(iface_ref) => {
			{
				let mut iface = iface_ref.get_mut().await;
				iface.value = new_value;
			}
			let iface = iface_ref.get().await;
			if let Err(err) = iface.value_changed(iface_ref.signal_emitter()).await {
				debug!(?err, path, "value_changed signal failed");
			}
		}
		Err(err) => debug!(?err, path, "lookup characteristic interface failed"),
	}
}

fn map(err: zbus::Error) -> Error {
	warn!(?err, "zbus error");
	Error::Unknown
}
