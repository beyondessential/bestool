//! The GATT transport: a byte stream carried by a characteristic the client writes and a
//! characteristic the device notifies on.
//!
//! Behaviour is specified in BLI-CHN, "Transport". This is the bottom layer, and the only part of the
//! device that knows about BlueZ. Everything above it — framing, the handshake, streams, messages —
//! sees an ordered byte stream and nothing else, which is what lets the same stack run over L2CAP
//! later, or in a browser over Web Bluetooth, without changing.

use std::{
	io,
	pin::Pin,
	sync::{Arc, Mutex},
	task::{Context, Poll},
};

use futures::{AsyncRead, AsyncWrite, SinkExt, StreamExt, channel::mpsc};

/// How many bytes to put in one notification. The negotiated attribute size is usually larger, but a
/// message is framed and reassembled above this layer, so a conservative chunk costs only a few extra
/// notifications and works on every peer.
const NOTIFY_CHUNK: usize = 20;

/// How many chunks to buffer in each direction before applying backpressure.
const QUEUE_DEPTH: usize = 64;

/// The client-write end, shared with the GATT characteristic callback so that bytes written by a
/// client reach whichever session is currently running.
#[derive(Clone, Default)]
pub struct InboundSink(Arc<Mutex<Option<mpsc::Sender<Vec<u8>>>>>);

impl InboundSink {
	/// Deliver bytes written by the client to the running session, if there is one. Bytes arriving
	/// with no session are dropped: a client that writes before subscribing has not opened a channel.
	pub fn deliver(&self, bytes: Vec<u8>) {
		let mut guard = self.0.lock().expect("inbound sink is not poisoned");
		if let Some(sender) = guard.as_mut() {
			if sender.try_send(bytes).is_err() {
				// The session has gone away, or is not keeping up. Either way the channel is done.
				*guard = None;
			}
		}
	}

	fn install(&self, sender: mpsc::Sender<Vec<u8>>) {
		*self.0.lock().expect("inbound sink is not poisoned") = Some(sender);
	}

	fn clear(&self) {
		*self.0.lock().expect("inbound sink is not poisoned") = None;
	}
}

/// A byte stream over the two characteristics.
pub struct GattTransport {
	inbound: mpsc::Receiver<Vec<u8>>,
	outbound: mpsc::Sender<Vec<u8>>,
	sink: InboundSink,
	pending: Vec<u8>,
	consumed: usize,
}

impl GattTransport {
	/// Open a transport for one session, returning it alongside the receiver a notifier task drains.
	pub fn open(sink: &InboundSink) -> (Self, mpsc::Receiver<Vec<u8>>) {
		let (inbound_tx, inbound_rx) = mpsc::channel(QUEUE_DEPTH);
		let (outbound_tx, outbound_rx) = mpsc::channel(QUEUE_DEPTH);
		sink.install(inbound_tx);
		(
			Self {
				inbound: inbound_rx,
				outbound: outbound_tx,
				sink: sink.clone(),
				pending: Vec::new(),
				consumed: 0,
			},
			outbound_rx,
		)
	}
}

impl Drop for GattTransport {
	fn drop(&mut self) {
		// A finished session must not keep receiving a later client's bytes.
		self.sink.clear();
	}
}

impl AsyncRead for GattTransport {
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
				// The client has gone: end of stream, which tears the session down cleanly.
				Poll::Ready(None) => return Poll::Ready(Ok(0)),
				Poll::Pending => return Poll::Pending,
			}
		}
	}
}

impl AsyncWrite for GattTransport {
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
		let chunk = &buf[..buf.len().min(NOTIFY_CHUNK)];
		this.outbound
			.start_send(chunk.to_vec())
			.map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
		Poll::Ready(Ok(chunk.len()))
	}

	fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		let this = self.get_mut();
		this.outbound
			.poll_flush_unpin(cx)
			.map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
	}

	fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
		let this = self.get_mut();
		this.outbound
			.poll_close_unpin(cx)
			.map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
	}
}

#[cfg(test)]
mod tests {
	use futures::{AsyncReadExt, AsyncWriteExt};

	use super::*;

	#[tokio::test]
	async fn bytes_written_by_a_client_are_read_from_the_transport() {
		let sink = InboundSink::default();
		let (mut transport, _outbound) = GattTransport::open(&sink);
		sink.deliver(b"hello".to_vec());

		let mut buf = [0u8; 5];
		transport.read_exact(&mut buf).await.unwrap();
		assert_eq!(&buf, b"hello");
	}

	#[tokio::test]
	async fn writes_are_chunked_for_notification() {
		let sink = InboundSink::default();
		let (mut transport, mut outbound) = GattTransport::open(&sink);

		let payload = vec![7u8; NOTIFY_CHUNK * 2 + 5];
		transport.write_all(&payload).await.unwrap();
		transport.flush().await.unwrap();
		drop(transport);

		let mut seen = Vec::new();
		while let Some(chunk) = outbound.next().await {
			assert!(
				chunk.len() <= NOTIFY_CHUNK,
				"a notification must fit the chunk"
			);
			seen.extend_from_slice(&chunk);
		}
		assert_eq!(seen, payload);
	}

	#[tokio::test]
	async fn a_finished_session_stops_receiving_client_bytes() {
		let sink = InboundSink::default();
		let (transport, _outbound) = GattTransport::open(&sink);
		drop(transport);
		// Delivering after the session ended must not panic, and must go nowhere.
		sink.deliver(b"late".to_vec());
	}

	#[tokio::test]
	async fn the_client_going_away_ends_the_stream() {
		let sink = InboundSink::default();
		let (mut transport, _outbound) = GattTransport::open(&sink);
		sink.clear();

		let mut buf = [0u8; 4];
		assert_eq!(transport.read(&mut buf).await.unwrap(), 0);
	}
}
