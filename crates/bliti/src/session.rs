//! One authenticated session: the handshake, the streams above it, and the messages they carry.
//!
//! This is written against any byte transport rather than against GATT, so the whole session can be
//! exercised over an in-memory duplex with no adapter involved, and so the same code serves a
//! different transport later without changing.
//!
//! Milestone one carries one thing in each direction (BLI-CHN, and the channel demonstration): text
//! from the client that the device prints, and the device's hostname and addresses, which it sends
//! when a client arrives and again whenever they change, without being asked.

use std::time::Duration;

use bliti_core::{
	channel::{
		messages::{ClientMessage, DeviceMessage, parse_client_message},
		stream::{Mode, Streams, accept_responder, multiplex, read_message, write_message},
	},
	key_schedule::StickerSecret,
};
use futures::{AsyncRead, AsyncWrite};

use crate::facts;

/// How often to look for a change in the device's addresses while a client is connected.
const ADDRESS_POLL: Duration = Duration::from_secs(2);

/// Run a session to completion over a transport, as the device.
///
/// Returns once the client goes away or the channel fails. A failed handshake is an ordinary outcome
/// rather than an error worth stopping the daemon for: anyone in range can connect and try, and the
/// device stays reachable by a legitimate operator afterwards (BLI-ADV, "Advertising continuously").
pub async fn run<S>(transport: S, secret: &StickerSecret) -> Result<(), SessionError>
where
	S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
	let encrypted = accept_responder(transport, secret)
		.await
		.map_err(|err| SessionError::Handshake(err.to_string()))?;
	tracing::info!("handshake complete");

	let (mut streams, driver) = multiplex(encrypted, Mode::Server);
	let driving = tokio::spawn(async move {
		if let Err(err) = driver.await {
			tracing::debug!(%err, "connection closed");
		}
	});

	let result = converse(&mut streams).await;
	driving.abort();
	result
}

/// The device's half of the conversation: report identity unsolicited, and serve whatever the client
/// opens.
async fn converse(streams: &mut Streams) -> Result<(), SessionError> {
	// The device speaks first, without being asked. This is the property the stream layer exists for.
	let mut reporting = streams
		.open()
		.await
		.map_err(|err| SessionError::Stream(err.to_string()))?;
	let mut last = facts::identity_message();
	write_message(&mut reporting, &last.to_json())
		.await
		.map_err(|err| SessionError::Stream(err.to_string()))?;

	let mut ticker = tokio::time::interval(ADDRESS_POLL);
	ticker.tick().await;

	loop {
		tokio::select! {
			// State that changes while a client is connected is sent as it happens.
			_ = ticker.tick() => {
				let current = facts::identity_message();
				if current != last {
					tracing::info!("device facts changed; reporting");
					write_message(&mut reporting, &current.to_json())
						.await
						.map_err(|err| SessionError::Stream(err.to_string()))?;
					last = current;
				}
			}

			inbound = streams.accept() => {
				let Some(mut stream) = inbound else {
					tracing::info!("client disconnected");
					return Ok(());
				};
				tokio::spawn(async move {
					if let Err(err) = serve_stream(&mut stream).await {
						tracing::debug!(%err, "stream ended");
					}
				});
			}
		}
	}
}

/// Serve one stream a client opened, until it closes. Closing it leaves the others and the connection
/// alive.
async fn serve_stream<S>(stream: &mut S) -> Result<(), SessionError>
where
	S: AsyncRead + AsyncWrite + Unpin,
{
	while let Some(raw) = read_message(stream)
		.await
		.map_err(|err| SessionError::Stream(err.to_string()))?
	{
		let reply = match parse_client_message(&raw) {
			Ok(ClientMessage::Text { text }) => {
				// The client-to-device direction, proved by the device printing what it was sent.
				// Standard output reaches the journal once the daemon runs as a service.
				println!("{text}");
				None
			}
			// A message the device does not understand is reported, and the channel stays open.
			Err(unknown) => Some(unknown),
		};
		if let Some(DeviceMessage::Unknown { reason }) = &reply {
			tracing::warn!(%reason, "message not understood");
		}
		if let Some(reply) = reply {
			write_message(stream, &reply.to_json())
				.await
				.map_err(|err| SessionError::Stream(err.to_string()))?;
		}
	}
	Ok(())
}

/// A failure within one session. None of these stops the daemon.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
	/// The handshake did not complete: a client that has not scanned this device's sticker, or noise
	/// on the link.
	#[error("handshake failed: {0}")]
	Handshake(String),

	/// A stream failed or the connection went away.
	#[error("stream: {0}")]
	Stream(String),
}

#[cfg(test)]
mod tests {
	use bliti_core::channel::stream::connect_initiator;
	use tokio_util::compat::TokioAsyncReadCompatExt;

	use super::*;

	fn secret(byte: u8) -> StickerSecret {
		StickerSecret::from_bytes([byte; 32])
	}

	/// Drive a device session against a client over an in-memory duplex, with no BLE involved.
	#[tokio::test]
	async fn a_client_reaches_the_device_and_is_told_who_it_is() {
		let psk = secret(0x42);
		let (client_side, device_side) = tokio::io::duplex(1 << 16);

		let device_psk = psk.clone();
		tokio::spawn(async move {
			let _ = run(device_side.compat(), &device_psk).await;
		});

		let encrypted = connect_initiator(client_side.compat(), &psk).await.unwrap();
		let (mut streams, driver) = multiplex(encrypted, Mode::Client);
		tokio::spawn(async move {
			let _ = driver.await;
		});

		// The device opens a stream and reports its identity without being asked.
		let mut reporting = streams.accept().await.expect("device reports unsolicited");
		let raw = read_message(&mut reporting).await.unwrap().unwrap();
		let message: DeviceMessage = serde_json::from_slice(&raw).unwrap();
		let DeviceMessage::Identity { hostname, .. } = message else {
			panic!("expected an identity message");
		};
		assert!(!hostname.is_empty());

		// And the client-to-device direction: a message the device does not understand is answered
		// rather than closing the channel, which also proves the channel is live in that direction.
		let mut stream = streams.open().await.unwrap();
		write_message(&mut stream, br#"{"type":"nonsense"}"#)
			.await
			.unwrap();
		let raw = read_message(&mut stream).await.unwrap().unwrap();
		let reply: DeviceMessage = serde_json::from_slice(&raw).unwrap();
		assert!(matches!(reply, DeviceMessage::Unknown { .. }));

		// The connection survives it: a further message is still served.
		write_message(&mut stream, br#"{"type":"also nonsense"}"#)
			.await
			.unwrap();
		let raw = read_message(&mut stream).await.unwrap().unwrap();
		let reply: DeviceMessage = serde_json::from_slice(&raw).unwrap();
		assert!(matches!(reply, DeviceMessage::Unknown { .. }));
	}

	#[tokio::test]
	async fn a_client_with_the_wrong_sticker_cannot_open_a_session() {
		let (client_side, device_side) = tokio::io::duplex(1 << 16);
		let device = tokio::spawn(async move { run(device_side.compat(), &secret(0x01)).await });

		// A client holding a different sticker fails the handshake, in both directions.
		assert!(
			connect_initiator(client_side.compat(), &secret(0x02))
				.await
				.is_err()
		);
		assert!(matches!(
			device.await.unwrap(),
			Err(SessionError::Handshake(_))
		));
	}
}
