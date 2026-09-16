//! The Noise `NNpsk0` handshake and the transport it produces.
//!
//! Behaviour is specified in BLI-CHN, "Authentication". Both ends bring only ephemeral keys and all
//! authentication comes from the pre-shared secret, so completing the handshake proves in both
//! directions that each end holds the sticker secret. The handshake produces a fresh session key
//! and gives the session forward secrecy, so recovering a sticker secret later does not decrypt a
//! recorded session.
//!
//! `snow`'s pure-Rust default resolver is used, so this builds for `wasm32-unknown-unknown` and the
//! web application runs the identical handshake.

use snow::{Builder, HandshakeState, TransportState};

use super::ChannelError;
use crate::key_schedule::StickerSecret;

/// The Noise protocol: `NNpsk0` over X25519, ChaCha20-Poly1305, and BLAKE2s. The PSK sits at
/// position 0, mixed in before the first message. This string is part of the wire contract; changing
/// it is incompatible with deployed peers.
pub const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";

/// The largest Noise transport message, including its authentication tag.
const MAX_NOISE_MESSAGE: usize = 65535;

/// The Poly1305 authentication tag length added to every transport message.
const TAG_LEN: usize = 16;

/// The largest plaintext that fits in one transport message.
pub const MAX_PLAINTEXT: usize = MAX_NOISE_MESSAGE - TAG_LEN;

/// An in-progress `NNpsk0` handshake.
///
/// `NNpsk0` is a two-message pattern: the initiator writes the first message, the responder reads it
/// and writes the second, and the initiator reads that. Both ends then move into transport mode.
pub struct Handshake {
	state: HandshakeState,
}

impl Handshake {
	/// Build the initiating side, keyed by the sticker secret. The client is the initiator.
	pub fn initiator(psk: &StickerSecret) -> Result<Self, ChannelError> {
		Self::build(psk, true)
	}

	/// Build the responding side, keyed by the sticker secret. The device is the responder.
	pub fn responder(psk: &StickerSecret) -> Result<Self, ChannelError> {
		Self::build(psk, false)
	}

	fn build(psk: &StickerSecret, initiator: bool) -> Result<Self, ChannelError> {
		let params = NOISE_PARAMS
			.parse()
			.map_err(|err| ChannelError::Handshake(format!("invalid Noise parameters: {err}")))?;
		let builder = Builder::new(params)
			.psk(0, psk.as_bytes())
			.map_err(|err| ChannelError::Handshake(err.to_string()))?;
		let state = if initiator {
			builder.build_initiator()
		} else {
			builder.build_responder()
		}
		.map_err(|err| ChannelError::Handshake(err.to_string()))?;
		Ok(Self { state })
	}

	/// Write the next handshake message, returning the bytes to send to the peer.
	pub fn write_message(&mut self) -> Result<Vec<u8>, ChannelError> {
		let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
		let len = self
			.state
			.write_message(&[], &mut buf)
			.map_err(|err| ChannelError::Handshake(err.to_string()))?;
		buf.truncate(len);
		Ok(buf)
	}

	/// Read a handshake message received from the peer.
	pub fn read_message(&mut self, message: &[u8]) -> Result<(), ChannelError> {
		let mut buf = vec![0u8; MAX_NOISE_MESSAGE];
		self.state
			.read_message(message, &mut buf)
			.map_err(|err| ChannelError::Handshake(err.to_string()))?;
		Ok(())
	}

	/// Whether the handshake has completed and can move into transport mode.
	pub fn is_finished(&self) -> bool {
		self.state.is_handshake_finished()
	}

	/// Move into transport mode once the handshake is finished.
	pub fn into_transport(self) -> Result<Transport, ChannelError> {
		let state = self
			.state
			.into_transport_mode()
			.map_err(|err| ChannelError::Handshake(err.to_string()))?;
		Ok(Transport { state })
	}
}

/// An established Noise transport: the encrypted, authenticated channel the handshake produces.
///
/// Each call encrypts or decrypts one Noise transport message. Above this sits the stream layer,
/// which chops its byte stream into pieces no larger than [`MAX_PLAINTEXT`].
pub struct Transport {
	state: TransportState,
}

impl Transport {
	/// Encrypt one plaintext into a transport message. The plaintext must be no larger than
	/// [`MAX_PLAINTEXT`].
	pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, ChannelError> {
		let mut buf = vec![0u8; plaintext.len() + TAG_LEN];
		let len = self
			.state
			.write_message(plaintext, &mut buf)
			.map_err(|err| ChannelError::Handshake(err.to_string()))?;
		buf.truncate(len);
		Ok(buf)
	}

	/// Decrypt one transport message into its plaintext. A message that fails authentication — a
	/// replay, a forgery, or corruption — surfaces as an error rather than plaintext.
	pub fn decrypt(&mut self, message: &[u8]) -> Result<Vec<u8>, ChannelError> {
		let mut buf = vec![0u8; message.len()];
		let len = self
			.state
			.read_message(message, &mut buf)
			.map_err(|err| ChannelError::Handshake(err.to_string()))?;
		buf.truncate(len);
		Ok(buf)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn secret(byte: u8) -> StickerSecret {
		StickerSecret::from_bytes([byte; 32])
	}

	/// Drive a full handshake between two ends holding the given secrets, returning their transports
	/// if it completes.
	fn run(
		client_psk: &StickerSecret,
		device_psk: &StickerSecret,
	) -> Result<(Transport, Transport), ChannelError> {
		let mut client = Handshake::initiator(client_psk)?;
		let mut device = Handshake::responder(device_psk)?;

		let msg1 = client.write_message()?;
		device.read_message(&msg1)?;
		let msg2 = device.write_message()?;
		client.read_message(&msg2)?;

		assert!(client.is_finished());
		assert!(device.is_finished());
		Ok((client.into_transport()?, device.into_transport()?))
	}

	#[test]
	fn matching_secret_completes_and_carries_messages() {
		let psk = secret(0xab);
		let (mut client, mut device) = run(&psk, &psk).unwrap();

		// Both directions carry traffic under the session key.
		let ct = client.encrypt(b"hello device").unwrap();
		assert_ne!(ct, b"hello device");
		assert_eq!(device.decrypt(&ct).unwrap(), b"hello device");

		let ct = device.encrypt(b"hello client").unwrap();
		assert_eq!(client.decrypt(&ct).unwrap(), b"hello client");
	}

	#[test]
	fn wrong_secret_fails_the_handshake() {
		// A client that scanned a different sticker cannot complete the handshake.
		let result = run(&secret(0x01), &secret(0x02));
		assert!(matches!(result, Err(ChannelError::Handshake(_))));
	}

	#[test]
	fn replayed_transport_message_is_rejected() {
		let psk = secret(0x7c);
		let (mut client, mut device) = run(&psk, &psk).unwrap();
		let first = client.encrypt(b"one").unwrap();
		let second = client.encrypt(b"two").unwrap();
		assert_eq!(device.decrypt(&first).unwrap(), b"one");
		// Replaying the first message out of order fails the nonce-bound authentication.
		assert!(device.decrypt(&first).is_err());
		// And the legitimate next message still decrypts, proving the failure is the replay.
		assert_eq!(device.decrypt(&second).unwrap(), b"two");
	}

	#[test]
	fn tampered_message_is_rejected() {
		let psk = secret(0x33);
		let (mut client, mut device) = run(&psk, &psk).unwrap();
		let mut ct = client.encrypt(b"authentic").unwrap();
		let last = ct.len() - 1;
		ct[last] ^= 0x01;
		assert!(device.decrypt(&ct).is_err());
	}
}
