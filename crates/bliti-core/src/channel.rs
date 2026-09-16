//! The authenticated channel: the layers that sit above GATT carrying reliable, ordered bytes.
//!
//! Behaviour is specified in `.workhorse/specs/bliti/channel.md` (BLI-CHN). Once a client has
//! matched a device by its handle, the two authenticate with a Noise `NNpsk0` handshake keyed by
//! the sticker secret, then carry application messages over the channel it establishes.
//!
//! The layers, each depending only on the one beneath it carrying bytes reliably and in order:
//!
//! | layer | module |
//! | --- | --- |
//! | framing | [`framing`] — message boundaries across the negotiated attribute size |
//! | Noise `NNpsk0` | [`noise`] — mutual authentication, encryption, a session key |
//! | stream multiplexing | (yamux, wired in with the daemon's async transport) |
//! | JSON | [`messages`] — application messages |
//!
//! This module carries the transport-agnostic pieces. The daemon binds them to `bluer`'s GATT and
//! the web application to Web Bluetooth; the pieces themselves neither know nor care which.

pub mod framing;
pub mod messages;
pub mod noise;
pub mod stream;

/// A failure in the channel below the application layer.
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
	/// The handshake could not be built or driven: a wrong sticker secret, a replayed or spoofed
	/// handshake, or a peer that does not hold the secret all surface here, because none can complete
	/// the `NNpsk0` handshake.
	#[error("handshake failed: {0}")]
	Handshake(String),

	/// A framed message exceeded the maximum a peer will buffer, so it is refused rather than let a
	/// peer in range exhaust memory by claiming a huge length.
	#[error("framed message of {claimed} bytes exceeds the {max}-byte maximum")]
	FrameTooLarge {
		/// The length the frame header claimed.
		claimed: usize,
		/// The largest message that will be buffered.
		max: usize,
	},
}
