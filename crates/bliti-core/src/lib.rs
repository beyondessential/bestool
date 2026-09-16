//! Protocol core for bliti, the QR-anchored BLE device provisioning protocol.
//!
//! bliti provisions headless devices over Bluetooth Low Energy, anchored to a QR sticker printed on
//! the device's enclosure. This crate is the protocol core: the derivation chain from a board ID to
//! a sticker secret to an advertised handle, the sticker payload, and the authenticated channel. It
//! carries no BlueZ and no hardware beyond the board-ID backends behind the `backends` feature, so
//! it unit-tests anywhere and compiles for `wasm32-unknown-unknown`, which is what lets the web
//! application share one implementation of the key schedule and the handshake with the device.
//!
//! Behaviour is specified under `.workhorse/specs/bliti/`; each module names the spec it implements.
//!
//! # Features
//!
//! - `derive` (default): the memory-hard sticker-secret derivation (argon2id). Needed by the device
//!   and the sticker generator, never by a client, and left out of wasm builds.
//! - `backends` (default): board-ID backends that read real firmware. Left out of wasm builds.

use uuid::Uuid;

pub mod advertisement;
pub mod board_id;
pub mod channel;
pub mod key_schedule;
pub mod sticker;

/// The 128-bit service UUID identifying a device as speaking bliti. It is advertised in the clear so
/// that a client can filter a scan on it (BLI-ADV), which on some client platforms is the only
/// filtering offered.
pub const SERVICE_UUID: Uuid = Uuid::from_u128(0x63c7f3bc_0599_4a66_bdcd_f28ec571c118);

/// The GATT characteristic the client writes to send bytes to the device (BLI-CHN, "Transport").
pub const CHARACTERISTIC_UUID_CLIENT_TX: Uuid =
	Uuid::from_u128(0x973bed6f_f4f9_4cae_b237_1b51701a77f5);

/// The GATT characteristic the device notifies on to send bytes to the client (BLI-CHN,
/// "Transport").
pub const CHARACTERISTIC_UUID_DEVICE_TX: Uuid =
	Uuid::from_u128(0xa7aabad6_3fc2_4c9b_953b_03a70a193ec4);
