//! The browser client for bliti: the protocol half of the web application (BLI-WEB).
//!
//! This crate compiles to wasm and carries everything the specs describe — reading a sticker
//! (BLI-STK), recomputing and matching the advertised handle (BLI-ADV), the `NNpsk0` handshake, the
//! stream layer, and the JSON messages (BLI-CHN). It is the same code the daemon and the
//! command-line client run, which is the point: one implementation of the key schedule and the
//! handshake rather than a Rust one and a JavaScript one that must agree forever.
//!
//! What stays in JavaScript is Web Bluetooth, the camera, and the interface. Those are browser APIs
//! with no protocol in them, and binding them through wasm would buy nothing.
//!
//! The memory-hard derivation of BLI-KEY never runs here: a client reads the sticker secret from the
//! payload and only computes the handle, which is a fast hash. The crate therefore takes `bliti-core`
//! without its default features, and argon2 is not in the build at all.

use std::{cell::RefCell, rc::Rc};

use bliti_core::{
	advertisement::Advertised,
	channel::{
		messages::ClientMessage,
		stream::{
			Mode, Stream, Streams, connect_initiator, multiplex, read_message, write_message,
		},
	},
	sticker::StickerPayload,
};
use futures::{channel::mpsc, lock::Mutex};
use js_sys::{Function, Promise};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{future_to_promise, spawn_local};

mod transport;

use transport::WebTransport;

/// Report panics to the console rather than as an unexplained trap. Called once by the application
/// as it starts.
#[wasm_bindgen]
pub fn start() {
	console_error_panic_hook::set_once();
}

/// The service UUID a client filters its scan by (BLI-ADV), in the lowercase hyphenated form the Web
/// Bluetooth API expects. Read from the core so the browser filters on the same UUID the device
/// advertises.
#[wasm_bindgen]
pub fn service_uuid() -> String {
	bliti_core::SERVICE_UUID.to_string()
}

/// The characteristic a client writes to send bytes to the device (BLI-CHN, "Transport").
#[wasm_bindgen]
pub fn client_tx_uuid() -> String {
	bliti_core::CHARACTERISTIC_UUID_CLIENT_TX.to_string()
}

/// The characteristic the device notifies on to send bytes to the client (BLI-CHN, "Transport").
#[wasm_bindgen]
pub fn device_tx_uuid() -> String {
	bliti_core::CHARACTERISTIC_UUID_DEVICE_TX.to_string()
}

/// A sticker the application has read, by either of the paths in BLI-WEB.
#[wasm_bindgen]
pub struct Sticker {
	payload: StickerPayload,
}

#[wasm_bindgen]
impl Sticker {
	/// Read a sticker however it was given: the URL a code encodes, the fragment alone, or the
	/// human-readable rendering printed beneath the code. All three carry the same payload, and a
	/// payload that parses as none of them is reported as unreadable.
	#[wasm_bindgen(constructor)]
	pub fn new(text: &str) -> Result<Sticker, JsError> {
		let text = text.trim();
		let payload = StickerPayload::from_url(text)
			.or_else(|_| StickerPayload::from_fragment(text))
			.or_else(|_| StickerPayload::from_human(text))
			.map_err(|err| JsError::new(&format!("this is not a bliti sticker: {err}")))?;
		Ok(Self { payload })
	}

	/// The version of the sticker in hand. A device advertising a different one is reported as being
	/// at a version this client does not support, rather than as not matching.
	#[wasm_bindgen(getter)]
	pub fn version(&self) -> u8 {
		self.payload.version()
	}

	/// The sticker's URL, as its code encodes it.
	#[wasm_bindgen(getter)]
	pub fn url(&self) -> String {
		self.payload.to_url()
	}

	/// The human-readable rendering printed beneath the code.
	#[wasm_bindgen(getter)]
	pub fn human(&self) -> String {
		self.payload.to_human()
	}

	/// Read a local name heard over the air against this sticker (BLI-ADV, "Matching").
	///
	/// `undefined` where the name is not a bliti payload at all, which is the ordinary case for every
	/// other device in range and is passed over rather than reported.
	pub fn read_local_name(&self, name: &str) -> Option<Advertisement> {
		Advertised::from_local_name(name).map(|advertised| Advertisement {
			version: advertised.version,
			// The version is checked by the caller before the match is believed: no two versions
			// produce a matching handle, so a mismatch there is not a different device.
			matches: advertised.version == self.payload.version()
				&& advertised.matches(self.payload.secret()),
		})
	}
}

/// What a client made of one device's advertisement.
#[wasm_bindgen]
pub struct Advertisement {
	version: u8,
	matches: bool,
}

#[wasm_bindgen]
impl Advertisement {
	/// The version the device advertises.
	#[wasm_bindgen(getter)]
	pub fn version(&self) -> u8 {
		self.version
	}

	/// Whether this device is the one the sticker belongs to.
	#[wasm_bindgen(getter)]
	pub fn matches(&self) -> bool {
		self.matches
	}
}

/// The state a channel holds once it is open. Kept behind a shared handle so the exported methods can
/// hand work to a task without borrowing across an await.
struct Inner {
	payload: StickerPayload,
	transport: RefCell<Option<WebTransport>>,
	inbound: RefCell<mpsc::Sender<Vec<u8>>>,
	// An async lock rather than a cell: it is held across opening a stream, and two sends in
	// flight queue behind each other rather than colliding over the handle.
	streams: Mutex<Option<Streams>>,
}

/// A channel to a device: the handshake of BLI-CHN and the streams above it.
///
/// Built before the connection is driven, so the application can start feeding it the device's
/// notifications before the handshake runs.
#[wasm_bindgen]
pub struct Channel {
	inner: Rc<Inner>,
}

#[wasm_bindgen]
impl Channel {
	/// Prepare a channel for a device, given the function that writes a chunk to the device's write
	/// characteristic. The handshake does not run until [`Channel::connect`] is called.
	#[wasm_bindgen(constructor)]
	pub fn new(sticker: &Sticker, write: Function) -> Channel {
		let (transport, inbound) = WebTransport::new(write);
		Channel {
			inner: Rc::new(Inner {
				payload: sticker.payload.clone(),
				transport: RefCell::new(Some(transport)),
				inbound: RefCell::new(inbound),
				streams: Mutex::new(None),
			}),
		}
	}

	/// Feed bytes arriving on the device's notify characteristic.
	pub fn receive(&self, bytes: &[u8]) {
		let _ = self.inner.inbound.borrow_mut().try_send(bytes.to_vec());
	}

	/// Run the handshake and take the device's first message, resolving to it as JSON.
	///
	/// The device speaks first, without being asked: it opens a stream and reports its identity. Every
	/// later report on that stream — the device sends one whenever its addresses change — is passed to
	/// `on_message`, and `on_closed` is called once the stream ends.
	pub fn connect(&self, on_message: Function, on_closed: Function) -> Promise {
		let inner = self.inner.clone();
		future_to_promise(async move {
			let transport = inner
				.transport
				.borrow_mut()
				.take()
				.ok_or_else(|| JsError::new("this channel has already been connected"))?;

			let encrypted = connect_initiator(transport, inner.payload.secret())
				.await
				.map_err(|err| JsError::new(&format!("handshake failed: {err}")))?;

			let (mut streams, driver) = multiplex(encrypted, Mode::Client);
			spawn_local(async move {
				let _ = driver.await;
			});

			let mut reporting = streams
				.accept()
				.await
				.ok_or_else(|| JsError::new("the device closed the channel before reporting"))?;
			let first = read_message(&mut reporting)
				.await
				.map_err(|err| JsError::new(&format!("reading the device's report: {err}")))?
				.ok_or_else(|| JsError::new("the device reported nothing"))?;

			// The device reports again whenever what it reported changes, so the stream is read for as
			// long as it lives rather than once.
			spawn_local(report_until_closed(reporting, on_message, on_closed));

			*inner.streams.lock().await = Some(streams);
			Ok(JsValue::from_str(&String::from_utf8_lossy(&first)))
		})
	}

	/// Send a line of text for the device to print, proving the client-to-device direction.
	pub fn send_text(&self, text: String) -> Promise {
		let inner = self.inner.clone();
		future_to_promise(async move {
			let mut streams = inner.streams.lock().await;
			let streams = streams
				.as_mut()
				.ok_or_else(|| JsError::new("this channel is not connected"))?;
			let mut stream = streams
				.open()
				.await
				.map_err(|err| JsError::new(&format!("opening a stream: {err}")))?;
			let message = ClientMessage::Text { text };
			write_message(&mut stream, &message.to_json())
				.await
				.map_err(|err| JsError::new(&format!("sending the text: {err}")))?;
			Ok(JsValue::UNDEFINED)
		})
	}
}

/// Pass every further message the device sends on its reporting stream to the application, then say
/// when the stream ends.
async fn report_until_closed(mut reporting: Stream, on_message: Function, on_closed: Function) {
	loop {
		match read_message(&mut reporting).await {
			Ok(Some(raw)) => {
				let _ = on_message.call1(
					&JsValue::NULL,
					&JsValue::from_str(&String::from_utf8_lossy(&raw)),
				);
			}
			Ok(None) => break,
			Err(err) => {
				let _ = on_closed.call1(&JsValue::NULL, &JsValue::from_str(&err.to_string()));
				return;
			}
		}
	}
	let _ = on_closed.call1(&JsValue::NULL, &JsValue::NULL);
}
