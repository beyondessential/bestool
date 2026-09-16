//! A byte stream over Web Bluetooth's two characteristics.
//!
//! The browser's mirror of the daemon's GATT transport and of the command-line client's: bytes the
//! device notifies come in, bytes the client writes go out, and everything above sees an ordered byte
//! stream and nothing else. Web Bluetooth itself stays in JavaScript — this only takes the bytes it
//! yields and the function it offers for writing them.

use std::{
	io,
	pin::Pin,
	task::{Context, Poll},
};

use futures::{AsyncRead, AsyncWrite, SinkExt, StreamExt, channel::mpsc};
use js_sys::{Function, Promise, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};

/// How many bytes to put in one write. Messages are framed and reassembled above this layer, so a
/// conservative chunk costs only extra writes and works against any negotiated attribute size. The
/// same figure the daemon and the command-line client use.
const WRITE_CHUNK: usize = 20;

/// How many chunks to buffer in each direction before applying backpressure.
const QUEUE_DEPTH: usize = 64;

/// A byte stream over the two characteristics.
pub struct WebTransport {
	inbound: mpsc::Receiver<Vec<u8>>,
	outbound: mpsc::Sender<Vec<u8>>,
	pending: Vec<u8>,
	consumed: usize,
}

impl WebTransport {
	/// Open a transport, returning it alongside the sender that feeds it bytes arriving from the
	/// device. `write` is the JavaScript function that writes a chunk to the device, returning a
	/// promise; writes are serialised through one task so they reach the device in order.
	pub fn new(write: Function) -> (Self, mpsc::Sender<Vec<u8>>) {
		let (inbound_tx, inbound_rx) = mpsc::channel(QUEUE_DEPTH);
		let (outbound_tx, mut outbound_rx) = mpsc::channel::<Vec<u8>>(QUEUE_DEPTH);

		spawn_local(async move {
			while let Some(chunk) = outbound_rx.next().await {
				let array = Uint8Array::from(&chunk[..]);
				let Ok(returned) = write.call1(&JsValue::NULL, &array) else {
					break;
				};
				// A write that hands back a promise is awaited, so the next chunk is not offered until
				// this one has left. Web Bluetooth rejects overlapping writes on one characteristic.
				match returned.dyn_into::<Promise>() {
					Ok(promise) => {
						if JsFuture::from(promise).await.is_err() {
							break;
						}
					}
					Err(_) => continue,
				}
			}
		});

		(
			Self {
				inbound: inbound_rx,
				outbound: outbound_tx,
				pending: Vec::new(),
				consumed: 0,
			},
			inbound_tx,
		)
	}
}

impl AsyncRead for WebTransport {
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
				// The device has gone: end of stream, which tears the session down cleanly.
				Poll::Ready(None) => return Poll::Ready(Ok(0)),
				Poll::Pending => return Poll::Pending,
			}
		}
	}
}

impl AsyncWrite for WebTransport {
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
