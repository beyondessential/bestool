//! The daemon: advertising, the GATT server, and the sessions they let a client open.
//!
//! This is the only module that talks to BlueZ. Behaviour is specified in BLI-ADV and BLI-CHN.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use bliti_core::{
	CHARACTERISTIC_UUID_CLIENT_TX, CHARACTERISTIC_UUID_DEVICE_TX, SERVICE_UUID,
	key_schedule::{RotationSalt, StickerSecret},
	sticker::StickerPayload,
};
use bluer::{
	adv::Advertisement,
	gatt::local::{
		Application, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
		CharacteristicWrite, CharacteristicWriteMethod, Service,
	},
};
use futures::{FutureExt, StreamExt};
use miette::{IntoDiagnostic, Result, WrapErr};

use crate::{
	SALT_ROTATION,
	advertise::{ScanPayload, local_name},
	gatt::{GattTransport, InboundSink},
	identity, session,
};

/// Run the daemon until interrupted.
pub async fn run(cache: &Path, adapter_name: Option<&str>) -> Result<()> {
	// Establish identity before touching Bluetooth: a board whose sticker is dead, or that this build
	// cannot derive for, must say so rather than advertise a handle nobody can match.
	let identity = identity::establish(cache)
		.into_diagnostic()
		.wrap_err("establishing this board's identity")?;
	if identity.derived {
		tracing::info!(source = %identity.kind, "derived this board's sticker secret");
	} else {
		tracing::info!(source = %identity.kind, "sticker secret is cached");
	}
	let secret = Arc::new(identity.secret);

	let session = bluer::Session::new().await.into_diagnostic()?;
	let adapter = match adapter_name {
		Some(name) => session.adapter(name).into_diagnostic()?,
		None => session.default_adapter().await.into_diagnostic()?,
	};
	adapter.set_powered(true).await.into_diagnostic()?;
	tracing::info!(adapter = %adapter.name(), address = %adapter.address().await.into_diagnostic()?, "adapter ready");

	let sink = InboundSink::default();
	let _application = adapter
		.serve_gatt_application(application(&sink, secret.clone()))
		.await
		.into_diagnostic()
		.wrap_err("registering the GATT application")?;

	// A device advertises whenever it is running, re-registering the advertisement each time the salt
	// rolls. Anyone in range can connect and begin a handshake that will fail; that is expected, and
	// there is no lockout, because someone in range could otherwise deny an operator their own device.
	let mut rotation = tokio::time::interval(SALT_ROTATION);
	// An interval yields its first tick immediately; take it here so the first salt lasts a full
	// period rather than being replaced the instant it is advertised.
	rotation.tick().await;
	let mut shutdown = std::pin::pin!(tokio::signal::ctrl_c());
	loop {
		let salt = random_salt();
		let handle = secret.handle(salt);
		let name = local_name(handle);
		let _advertisement = adapter
			.advertise(advertisement(handle, salt, &name))
			.await
			.into_diagnostic()
			.wrap_err("registering the advertisement")?;
		tracing::info!(local_name = %name, "advertising");

		tokio::select! {
			_ = rotation.tick() => continue,
			result = &mut shutdown => {
				result.into_diagnostic()?;
				tracing::info!("stopping");
				return Ok(());
			}
		}
	}
}

/// A fresh rotation salt. Advertised in the clear; what it buys is that a passive observer cannot
/// follow a device by its handle across a change.
fn random_salt() -> RotationSalt {
	RotationSalt::from_bytes(rand::random())
}

/// The advertisement and its scan response.
///
/// The service UUID goes in the advertisement rather than the scan response, because filtering a scan
/// by service UUID is the only filtering some client platforms offer and it is applied to the
/// advertisement. The handle, salt and version ride in service data, which lands in the scan
/// response. Client platforms present the two to an application as one set of advertised data.
fn advertisement(
	handle: bliti_core::key_schedule::Handle,
	salt: RotationSalt,
	name: &str,
) -> Advertisement {
	let mut service_data = BTreeMap::new();
	service_data.insert(
		SERVICE_UUID,
		ScanPayload::new(handle, salt).to_service_data(),
	);

	Advertisement {
		advertisement_type: bluer::adv::Type::Peripheral,
		service_uuids: [SERVICE_UUID].into_iter().collect(),
		service_data,
		local_name: Some(name.to_owned()),
		discoverable: Some(true),
		..Default::default()
	}
}

/// The GATT application: one service with the characteristic a client writes and the one the device
/// notifies on.
fn application(sink: &InboundSink, secret: Arc<StickerSecret>) -> Application {
	let write_sink = sink.clone();
	let notify_sink = sink.clone();

	Application {
		services: vec![Service {
			uuid: SERVICE_UUID,
			primary: true,
			characteristics: vec![
				Characteristic {
					uuid: CHARACTERISTIC_UUID_CLIENT_TX,
					write: Some(CharacteristicWrite {
						write: true,
						write_without_response: true,
						method: CharacteristicWriteMethod::Fun(Box::new(move |bytes, _req| {
							let sink = write_sink.clone();
							async move {
								sink.deliver(bytes);
								Ok(())
							}
							.boxed()
						})),
						..Default::default()
					}),
					..Default::default()
				},
				Characteristic {
					uuid: CHARACTERISTIC_UUID_DEVICE_TX,
					notify: Some(CharacteristicNotify {
						notify: true,
						method: CharacteristicNotifyMethod::Fun(Box::new(move |mut notifier| {
							// A client subscribing is what opens a session: it is the point at which
							// the device can send, so it is the point at which a handshake can run.
							let sink = notify_sink.clone();
							let secret = secret.clone();
							async move {
								tracing::info!("client subscribed; opening a session");
								let (transport, mut outbound) = GattTransport::open(&sink);

								// Pump the device's bytes out as notifications for as long as the
								// client is subscribed.
								let pump = tokio::spawn(async move {
									while let Some(chunk) = outbound.next().await {
										if notifier.notify(chunk).await.is_err() {
											break;
										}
									}
								});

								// A failed handshake is an ordinary outcome: anyone in range can
								// connect and try, and the device stays reachable afterwards.
								match session::run(transport, &secret).await {
									Ok(()) => tracing::info!("session ended"),
									Err(err) => tracing::info!(%err, "session ended"),
								}
								pump.abort();
							}
							.boxed()
						})),
						..Default::default()
					}),
					..Default::default()
				},
			],
			..Default::default()
		}],
		..Default::default()
	}
}

/// Scan for bliti devices and report which one the sticker in hand belongs to.
///
/// This is the client half of BLI-ADV: recompute the handle from the sticker against whatever salt
/// each device advertises, and compare. It exists so discovery and matching can be exercised without
/// a browser; the web application does the same thing.
pub async fn scan(
	payload: &StickerPayload,
	seconds: u64,
	adapter_name: Option<&str>,
) -> Result<()> {
	let session = bluer::Session::new().await.into_diagnostic()?;
	let adapter = match adapter_name {
		Some(name) => session.adapter(name).into_diagnostic()?,
		None => session.default_adapter().await.into_diagnostic()?,
	};
	adapter.set_powered(true).await.into_diagnostic()?;

	let mut events = adapter.discover_devices().await.into_diagnostic()?;
	let deadline = tokio::time::sleep(std::time::Duration::from_secs(seconds));
	let mut deadline = std::pin::pin!(deadline);
	let mut matched = 0usize;
	let mut seen = std::collections::BTreeSet::new();

	tracing::info!(seconds, "scanning");
	loop {
		let event = tokio::select! {
			_ = &mut deadline => break,
			event = events.next() => match event {
				Some(event) => event,
				None => break,
			},
		};
		let bluer::AdapterEvent::DeviceAdded(address) = event else {
			continue;
		};
		if !seen.insert(address) {
			continue;
		}
		let device = adapter.device(address).into_diagnostic()?;
		let name = device.name().await.ok().flatten();
		let uuids = device.uuids().await.ok().flatten();
		let carries_bliti = uuids
			.as_ref()
			.is_some_and(|uuids| uuids.contains(&SERVICE_UUID));
		// Reporting what was heard, and not only what matched, is what makes a device that is
		// advertising the wrong shape distinguishable from one that is not advertising at all.
		tracing::debug!(%address, ?name, bliti = carries_bliti, "heard");
		let Ok(Some(service_data)) = device.service_data().await else {
			if carries_bliti {
				println!(
					"{address}  a bliti device advertising no service data (name {})",
					name.unwrap_or_else(|| "-".to_owned())
				);
			}
			continue;
		};
		let Some(raw) = service_data.get(&SERVICE_UUID) else {
			continue;
		};
		let Some(advertised) = ScanPayload::parse(raw) else {
			tracing::warn!(%address, "bliti service data of an unexpected shape");
			continue;
		};

		// The version is read before recomputing, so a device speaking a version this client does not
		// hold is reported as exactly that rather than as a device that simply did not match.
		if advertised.version != payload.version() {
			println!(
				"{address}  a bliti device at unsupported version {}",
				advertised.version
			);
			continue;
		}

		if payload.secret().handle(advertised.salt) == advertised.handle {
			matched += 1;
			println!(
				"{address}  MATCHES the sticker (local name {})",
				local_name(advertised.handle)
			);
		} else {
			println!("{address}  another bliti device");
		}
	}

	if matched == 0 {
		tracing::warn!("no device matching that sticker was heard");
	} else if matched > 1 {
		// Two devices answering one sticker is a handle collision, or a device being impersonated;
		// either way it is reported rather than silently picking one.
		tracing::warn!(matched, "more than one device matched that sticker");
	}
	Ok(())
}
