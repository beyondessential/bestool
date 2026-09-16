//! The daemon: advertising, the GATT server, and the sessions they let a client open.
//!
//! This is the only module that talks to BlueZ. Behaviour is specified in BLI-ADV and BLI-CHN.

use std::{path::Path, sync::Arc};

use bliti_core::{
	CHARACTERISTIC_UUID_CLIENT_TX, CHARACTERISTIC_UUID_DEVICE_TX, SERVICE_UUID,
	advertisement::Advertised,
	key_schedule::{Handle, RotationSalt, StickerSecret},
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
	// A legacy controller stops advertising the instant a client connects and does not resume when it
	// leaves: the advertisement stays registered with BlueZ, so nothing reports an error, but nothing
	// goes out and the device is undiscoverable to the next client. A session fires this when it ends,
	// and the loop re-registers the advertisement in response, which is what puts the device back on
	// the air.
	let readvertise = Arc::new(tokio::sync::Notify::new());
	let _application = adapter
		.serve_gatt_application(application(&sink, secret.clone(), readvertise.clone()))
		.await
		.into_diagnostic()
		.wrap_err("registering the GATT application")?;

	// A device advertises whenever it is running, re-registering the advertisement each time the salt
	// rolls and each time a session ends. Anyone in range can connect and begin a handshake that will
	// fail; that is expected, and there is no lockout, because someone in range could otherwise deny an
	// operator their own device.
	let mut rotation = tokio::time::interval(SALT_ROTATION);
	// An interval yields its first tick immediately; take it here so the first salt lasts a full
	// period rather than being replaced the instant it is advertised.
	rotation.tick().await;
	let mut shutdown = std::pin::pin!(tokio::signal::ctrl_c());
	loop {
		let salt = random_salt();
		let advertised = Advertised::new(secret.handle(salt), salt);
		let _advertisement = adapter
			.advertise(advertisement(advertised))
			.await
			.into_diagnostic()
			.wrap_err("registering the advertisement")?;
		tracing::info!(local_name = %advertised.to_local_name(), "advertising");

		tokio::select! {
			_ = rotation.tick() => continue,
			// A session just ended, so the controller has stopped advertising: drop this advertisement
			// and register a fresh one, which resumes it. A fresh salt comes with it, which is harmless.
			_ = readvertise.notified() => {
				tracing::info!("a session ended; resuming advertising");
				continue;
			}
			result = &mut shutdown => {
				result.into_diagnostic()?;
				tracing::info!("stopping");
				return Ok(());
			}
		}
	}
}

/// How often to check whether the client is still subscribed, while it is sending nothing.
const UNSUBSCRIBE_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// A handle rendered for a person to read in a log line.
fn hex(handle: Handle) -> String {
	handle
		.as_bytes()
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect()
}

/// A fresh rotation salt. Advertised in the clear; what it buys is that a passive observer cannot
/// follow a device by its handle across a change.
fn random_salt() -> RotationSalt {
	RotationSalt::from_bytes(rand::random())
}

/// The advertisement a device registers.
///
/// The service UUID goes in the advertisement, because filtering a scan by service UUID is the only
/// filtering some client platforms offer and it is applied to the advertisement. The handle, salt and
/// version ride in the local name, which is the one element a host will place in the scan response,
/// and so the only way the whole thing fits a controller that does only legacy advertising.
fn advertisement(advertised: Advertised) -> Advertisement {
	Advertisement {
		advertisement_type: bluer::adv::Type::Peripheral,
		service_uuids: [SERVICE_UUID].into_iter().collect(),
		local_name: Some(advertised.to_local_name()),
		discoverable: Some(true),
		// A short interval so a scanning client finds the device quickly and reliably. The default is
		// over a second, which on a legacy controller leaves a client's scan window catching an
		// advertisement only now and then, so the first attempt after a device comes on the air often
		// hears nothing. These are hints the controller rounds to what it supports.
		min_interval: Some(std::time::Duration::from_millis(100)),
		max_interval: Some(std::time::Duration::from_millis(150)),
		..Default::default()
	}
}

/// The GATT application: one service with the characteristic a client writes and the one the device
/// notifies on.
///
/// `readvertise` is fired whenever a session ends, so the daemon can resume advertising: a client
/// connecting stops the controller advertising, and only re-registering the advertisement brings it
/// back.
fn application(
	sink: &InboundSink,
	secret: Arc<StickerSecret>,
	readvertise: Arc<tokio::sync::Notify>,
) -> Application {
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
							let readvertise = readvertise.clone();
							async move {
								tracing::info!("client subscribed; opening a session");
								let (transport, mut outbound) = GattTransport::open(&sink);

								// Pump the device's bytes out as notifications for as long as the
								// client is subscribed, and notice when it stops being subscribed.
								//
								// Noticing matters: nothing else tells the device the client has gone.
								// The session reads until its transport ends, and the transport only
								// ends when the session drops it, so without this the two wait on each
								// other and the device stays busy with a client that left.
								let (left, gone) = tokio::sync::oneshot::channel();
								let pump = tokio::spawn(async move {
									loop {
										tokio::select! {
											chunk = outbound.next() => {
												let Some(chunk) = chunk else { break };
												if notifier.notify(chunk).await.is_err() {
													break;
												}
											}
											_ = tokio::time::sleep(UNSUBSCRIBE_POLL) => {
												if notifier.is_stopped() {
													break;
												}
											}
										}
									}
									let _ = left.send(());
								});

								// A failed handshake is an ordinary outcome: anyone in range can
								// connect and try, and the device stays reachable afterwards.
								tokio::select! {
									result = session::run(transport, &secret) => match result {
										Ok(()) => tracing::info!("session ended"),
										Err(err) => tracing::info!(%err, "session ended"),
									},
									_ = gone => tracing::info!("client unsubscribed; session ended"),
								}
								pump.abort();
								// The controller stopped advertising when this client connected. Now the
								// session is over, ask the daemon to put the device back on the air.
								readvertise.notify_one();
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
		let carries_bliti = device
			.uuids()
			.await
			.ok()
			.flatten()
			.is_some_and(|uuids| uuids.contains(&SERVICE_UUID));
		tracing::debug!(%address, ?name, bliti = carries_bliti, "heard");

		// A name that is not a bliti payload belongs to a device that is not one.
		let Some(advertised) = name.as_deref().and_then(Advertised::from_local_name) else {
			if carries_bliti {
				println!(
					"{address}  a bliti device whose name is not a payload ({})",
					name.unwrap_or_else(|| "-".to_owned())
				);
			}
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

		if advertised.matches(payload.secret()) {
			matched += 1;
			println!(
				"{address}  MATCHES the sticker (handle {})",
				hex(advertised.handle)
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
