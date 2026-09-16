//! The client half of the channel: connect to a device over GATT and run a session against it.
//!
//! This exists so the whole of BLI-CHN can be exercised from the command line, against a real device
//! over real BLE, without a browser. The web application does the same thing through Web Bluetooth,
//! and both sit on the same [`bliti_core::channel`] stack, so what this proves the browser inherits.

use std::{
	io,
	pin::Pin,
	task::{Context, Poll},
};

use bliti_core::{
	CHARACTERISTIC_UUID_CLIENT_TX, CHARACTERISTIC_UUID_DEVICE_TX, SERVICE_UUID,
	advertisement::Advertised,
	channel::{
		messages::{ClientMessage, DeviceMessage},
		stream::{Mode, connect_initiator, multiplex, read_message, write_message},
	},
	key_schedule::StickerSecret,
};
use bluer::gatt::remote::Characteristic;
use futures::{AsyncRead, AsyncWrite, SinkExt, StreamExt, channel::mpsc};
use miette::{IntoDiagnostic, Result, WrapErr, miette};

/// How long to wait for the host to discover what the peer offers, after the link is up.
const SERVICE_RESOLUTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// How many bytes to put in one write. Messages are framed and reassembled above this, so a
/// conservative chunk costs only extra writes and works against any negotiated attribute size.
const WRITE_CHUNK: usize = 20;

/// A byte stream over a device's two characteristics: notifications in, writes out.
///
/// The mirror image of the device's own transport, and the same shape as far as everything above is
/// concerned.
struct GattClientTransport {
	inbound: mpsc::Receiver<Vec<u8>>,
	outbound: mpsc::Sender<Vec<u8>>,
	pending: Vec<u8>,
	consumed: usize,
}

impl AsyncRead for GattClientTransport {
	fn poll_read(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &mut [u8],
	) -> Poll<io::Result<usize>> {
		let this = self.get_mut();
		loop {
			if this.consumed < this.pending.len() {
				let available = &this.pending[this.consumed..];
				let n = available.len().min(buf.len());
				buf[..n].copy_from_slice(&available[..n]);
				this.consumed += n;
				if this.consumed == this.pending.len() {
					this.pending.clear();
					this.consumed = 0;
				}
				return Poll::Ready(Ok(n));
			}
			match this.inbound.poll_next_unpin(cx) {
				Poll::Ready(Some(chunk)) => {
					this.pending = chunk;
					this.consumed = 0;
				}
				Poll::Ready(None) => return Poll::Ready(Ok(0)),
				Poll::Pending => return Poll::Pending,
			}
		}
	}
}

impl AsyncWrite for GattClientTransport {
	fn poll_write(
		self: Pin<&mut Self>,
		cx: &mut Context<'_>,
		buf: &[u8],
	) -> Poll<io::Result<usize>> {
		let this = self.get_mut();
		if buf.is_empty() {
			return Poll::Ready(Ok(0));
		}
		match this.outbound.poll_ready(cx) {
			Poll::Ready(Ok(())) => {}
			Poll::Ready(Err(_)) => return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
			Poll::Pending => return Poll::Pending,
		}
		let chunk = &buf[..buf.len().min(WRITE_CHUNK)];
		this.outbound
			.start_send(chunk.to_vec())
			.map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
		Poll::Ready(Ok(chunk.len()))
	}

	fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		self.get_mut()
			.outbound
			.poll_flush_unpin(cx)
			.map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
	}

	fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		self.get_mut()
			.outbound
			.poll_close_unpin(cx)
			.map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
	}
}

/// Find the bliti service's two characteristics on a connected device.
async fn characteristics(device: &bluer::Device) -> Result<(Characteristic, Characteristic)> {
	for service in device.services().await.into_diagnostic()? {
		if service.uuid().await.into_diagnostic()? != SERVICE_UUID {
			continue;
		}
		let (mut to_device, mut from_device) = (None, None);
		for characteristic in service.characteristics().await.into_diagnostic()? {
			match characteristic.uuid().await.into_diagnostic()? {
				u if u == CHARACTERISTIC_UUID_CLIENT_TX => to_device = Some(characteristic),
				u if u == CHARACTERISTIC_UUID_DEVICE_TX => from_device = Some(characteristic),
				_ => {}
			}
		}
		if let (Some(to_device), Some(from_device)) = (to_device, from_device) {
			return Ok((to_device, from_device));
		}
	}
	Err(miette!("the device does not carry the bliti service"))
}

/// Find the device a sticker belongs to, by the matching of BLI-ADV.
///
/// A client cannot reach a device it has not heard: a peer has to be discovered before it can be
/// connected to. So finding it is part of connecting, and this is the same scan-then-match the web
/// application performs before it opens a channel.
async fn find(
	adapter: &bluer::Adapter,
	secret: &StickerSecret,
	seconds: u64,
) -> Result<bluer::Address> {
	// Discovery has to be running for names to be refreshed, but the events it emits are not enough
	// on their own: a device the host already knows is announced once, carrying whatever name it was
	// last seen with, which after a salt roll is a handle that no longer matches. So the names of
	// every device known are re-read while the scan runs, rather than read once when it is announced.
	let _discovery = adapter.discover_devices().await.into_diagnostic()?;
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(seconds);

	while tokio::time::Instant::now() < deadline {
		for address in adapter.device_addresses().await.into_diagnostic()? {
			let device = adapter.device(address).into_diagnostic()?;
			let Some(name) = device.name().await.ok().flatten() else {
				continue;
			};
			let Some(advertised) = Advertised::from_local_name(&name) else {
				continue;
			};
			if advertised.version != bliti_core::key_schedule::VERSION {
				tracing::warn!(
					%address,
					version = advertised.version,
					"a bliti device at an unsupported version"
				);
				continue;
			}
			if advertised.matches(secret) {
				tracing::info!(%address, "matched the sticker");
				return Ok(address);
			}
		}
		tokio::time::sleep(std::time::Duration::from_secs(3)).await;
	}
	Err(miette!("no device matching that sticker was heard"))
}

/// Connect to a device, run the handshake, and exchange the milestone's two messages.
pub async fn connect(
	address: Option<bluer::Address>,
	secret: &StickerSecret,
	text: &str,
	adapter_name: Option<&str>,
) -> Result<()> {
	let session = bluer::Session::new().await.into_diagnostic()?;
	let adapter = match adapter_name {
		Some(name) => session.adapter(name).into_diagnostic()?,
		None => session.default_adapter().await.into_diagnostic()?,
	};
	adapter.set_powered(true).await.into_diagnostic()?;

	// A host keeps what it learned about a peer, and what it kept can be stale: a device it has
	// connected to before may never resolve its services again. Forgetting it first costs one
	// discovery and makes a connection attempt behave the same every time.
	let mut device = None;
	for attempt in 0..2 {
		let found = match address {
			Some(address) => address,
			None => find(&adapter, secret, 20).await?,
		};
		let candidate = adapter.device(found).into_diagnostic()?;
		if !candidate.is_connected().await.into_diagnostic()? {
			candidate
				.connect()
				.await
				.into_diagnostic()
				.wrap_err("connecting to the device")?;
		}
		tracing::info!(address = %found, "connected");

		// Connecting is not the same as knowing what the peer offers: the host discovers the peer's
		// attributes after the link is up, and until it has there are no services to look through.
		let deadline = tokio::time::Instant::now() + SERVICE_RESOLUTION_TIMEOUT;
		let mut resolved = false;
		while tokio::time::Instant::now() < deadline {
			if candidate.is_services_resolved().await.into_diagnostic()? {
				resolved = true;
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(200)).await;
		}

		if resolved {
			device = Some(candidate);
			break;
		}
		if attempt == 0 {
			tracing::warn!("services did not resolve; forgetting the device and trying once more");
			let _ = candidate.disconnect().await;
			let _ = adapter.remove_device(found).await;
			tokio::time::sleep(std::time::Duration::from_secs(1)).await;
		}
	}
	let device = device.ok_or_else(|| miette!("the device's services were never resolved"))?;

	let (to_device, from_device) = characteristics(&device).await?;
	let notifications = from_device.notify().await.into_diagnostic()?;

	// Pump notifications in and writes out, so the transport sees an ordinary byte stream.
	let (mut inbound_tx, inbound_rx) = mpsc::channel(64);
	let (outbound_tx, mut outbound_rx) = mpsc::channel::<Vec<u8>>(64);
	tokio::spawn(async move {
		let mut notifications = std::pin::pin!(notifications);
		while let Some(chunk) = notifications.next().await {
			if inbound_tx.try_send(chunk).is_err() {
				break;
			}
		}
	});
	tokio::spawn(async move {
		while let Some(chunk) = outbound_rx.next().await {
			if to_device.write(&chunk).await.is_err() {
				break;
			}
		}
	});

	let transport = GattClientTransport {
		inbound: inbound_rx,
		outbound: outbound_tx,
		pending: Vec::new(),
		consumed: 0,
	};

	// Everything from here is the same stack the browser will run.
	let encrypted = connect_initiator(transport, secret)
		.await
		.map_err(|err| miette!("handshake failed: {err}"))?;
	tracing::info!("handshake complete");

	let (mut streams, driver) = multiplex(encrypted, Mode::Client);
	tokio::spawn(async move {
		let _ = driver.await;
	});

	// The device speaks first, without being asked.
	let reporting = tokio::time::timeout(std::time::Duration::from_secs(20), streams.accept())
		.await
		.map_err(|_| miette!("the device did not report its identity"))?;
	let mut reporting = reporting.ok_or_else(|| miette!("the connection closed"))?;
	let raw = read_message(&mut reporting)
		.await
		.into_diagnostic()?
		.ok_or_else(|| miette!("no identity message"))?;
	match serde_json::from_slice(&raw).into_diagnostic()? {
		DeviceMessage::Identity {
			hostname,
			addresses,
		} => {
			println!("hostname: {hostname}");
			for address in addresses {
				println!("address:  {} on {}", address.address, address.interface);
			}
		}
		other => println!("device said: {other:?}"),
	}

	// And the other direction: text the device prints.
	let mut stream = streams.open().await.into_diagnostic()?;
	let message = ClientMessage::Text {
		text: text.to_owned(),
	};
	write_message(&mut stream, &message.to_json())
		.await
		.into_diagnostic()?;
	println!("sent:     {text}");

	// Let the writes drain and the device print before dropping the connection. Nothing acknowledges
	// a line of text, so waiting is the only way to know it had the chance.
	tokio::time::sleep(std::time::Duration::from_secs(3)).await;
	let _ = device.disconnect().await;
	Ok(())
}
