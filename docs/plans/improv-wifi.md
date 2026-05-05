# improv-wifi: BLE peripheral implementation for Linux

## Background

[Improv Wi-Fi](https://www.improv-wifi.com/) is a small open standard for
provisioning Wi-Fi credentials over BLE. The web client (Chrome/Edge with Web
Bluetooth) and the Android/iOS apps already exist; this work implements the
*device side* for embedded Linux targets — primarily Raspberry Pi systems
running our `iti` stack.

There is a stale branch `feat/iti/improv-wifi` (last touched 2024-05) that
sketched a crate. This plan absorbs what is reusable, completes the protocol,
wires up a real backend, and integrates it into `bestool`.

## State of the existing branch

The branch sits ~2 years behind `main`. The relevant artefact is a six-file
crate `crates/improv-wifi/` with the following content:

### Reusable (keep)

- Service / characteristic UUID constants — match the spec.
- `Status` enum (`AuthorizationRequired`/`Authorized`/`Provisioning`/`Provisioned`).
- `Error` enum (`InvalidRPC`/`UnknownRPC`/`UnableToConnect`/`NotAuthorized`/`Unknown`).
- BlueZ GATT scaffold using `bluer` (`Application` / `Service` / `Characteristic`
  with read / write / notify hooks).
- State architecture: `Arc<RwLock<InnerState>>` plus tokio broadcast channels
  to fan out status / error change notifications to BLE notify handlers.
- `WifiConfigurator` trait shape: per-backend impl with `can_authorize`,
  `can_identify`, `provision`.
- Skeleton RPC parser using `winnow`.

### Incomplete / wrong (rework)

1. RPC parser only knows commands `0x01` (Send Wi-Fi Settings) and `0x02`
   (Identify); spec defines `0x03`-`0x06`. The parser also doesn't decode the
   Wi-Fi settings *payload* (`[ssid_len][ssid][pwd_len][pwd]`).
2. `inner_handle_raw_rpc` is a `todo!()`.
3. RPC Result characteristic is registered but has no read/notify implementation
   — clients can never read responses.
4. `Error` enum is missing `BadHostname = 0x05` (added in spec since branch was
   written).
5. Capabilities byte only sets bit 0 (identify); spec defines bits 1-3 too
   (device info / scan / hostname).
6. **No BLE advertising** — the GATT app is registered with bluez but the
   adapter is never told to advertise the service UUID + service data, so
   nothing on the client side can discover the device. This is the most
   load-bearing missing piece.
7. Authorization timeout field exists but no logic enforces it (spec suggests
   ~60 s before reverting `Authorized` → `AuthorizationRequired`).
8. No multi-packet RPC reassembly. BLE writes are bounded by the negotiated MTU
   (≥23, often 20-byte payload by default); the spec explicitly says
   implementations must accept payloads spanning multiple writes.
9. No backend implementations behind `WifiConfigurator`. The `networkmanager`
   feature flag is declared but the impl is absent.
10. No `Identify` plumbing — the trait has `can_identify` but no `identify`
    method.
11. No CLI integration in `bestool`.
12. No tests.
13. Dependencies and edition are stale: `edition = "2021"`, `bluer 0.17`,
    `tokio 1.37`, `thiserror 1`, `winnow 0.6.8`. Workspace is on edition 2024,
    `thiserror 2`, etc.
14. Branch had unused `tokio::sync::RwLock` over what is essentially per-call
    locks — fine to keep, but the `// TODO: pro-actively write to the client`
    comments reflect that the notify wiring is half-done (the broadcast
    channels exist, the receiver loops in `CharacteristicNotify::Fun` exist,
    but they only fire when `modify_status` / `set_error` are called, and a
    few state mutations bypass those helpers).

## Verdict

The scaffold is worth salvaging — the bluer GATT setup is the bulk of the
"figure out the unfamiliar API" work and the protocol constants don't change.
We will rebase the crate skeleton onto current `main`, refresh deps, and
complete the protocol. We will *not* try to revive the WIP commits piecemeal;
they're noisy ("wip: rpc", "not sure how that works yet") and content-equivalent
across the two divergent bookmark heads. Squash and move on.

## Design

### Crate layout (`crates/improv-wifi/`)

A standalone, publishable crate (the existing branch already set
`description`/`keywords`/`categories` for crates.io — keep that). Re-export it
from `bestool` via a workspace dep, behind an `iti-improv-wifi` feature.

```
crates/improv-wifi/
  Cargo.toml
  src/
    lib.rs            # public API: ImprovWifi, WifiConfigurator trait, run loop
    advertising.rs    # bluer Advertisement registration with service data
    error.rs          # Error enum (extended with BadHostname)
    state.rs          # Status enum, InnerState, broadcast wiring
    rpc/
      mod.rs          # Rpc struct, Command enum, packet-level parser
      parse.rs        # winnow parsers for each command's payload
      result.rs       # encoder for RPC Result (length-prefixed string list + checksum)
      reassembly.rs   # multi-packet write buffering
    gatt.rs           # GATT Application builder (was lib.rs::install)
    timeout.rs        # authorization timeout task
    backends/
      mod.rs          # WifiConfigurator trait
      networkmanager.rs  # default backend, behind `networkmanager` feature
```

### Public API sketch

```rust
pub trait WifiConfigurator: Send + Sync + 'static {
    fn capabilities(&self) -> Capabilities;
    async fn identify(&self) -> Result<(), Error> { Ok(()) }
    async fn device_info(&self) -> Result<DeviceInfo, Error>;
    async fn scan(&self) -> Result<Vec<Network>, Error>;
    async fn get_hostname(&self) -> Result<String, Error>;
    async fn set_hostname(&self, name: &str) -> Result<(), Error>;
    async fn provision(&self, ssid: &str, password: &str) -> Result<Vec<String>, Error>;
    /// Optional: physical/UI authorization (button press, etc.).
    /// If `None`, device starts in `Authorized` and `Capabilities::can_authorize` is false.
    fn authorize_signal(&self) -> Option<tokio::sync::watch::Receiver<bool>> { None }
}

pub struct ImprovWifi<T> { /* ... */ }

impl<T: WifiConfigurator> ImprovWifi<T> {
    pub async fn install(adapter: &Adapter, configurator: T) -> Result<Self>;
    pub fn set_authorization_timeout(&mut self, t: Duration);
    /// Drives the service: GATT, advertising, timeout, authorize signal.
    pub async fn run(self) -> Result<()>;
}
```

`provision` returns a list of strings (typically a redirect URL for the new
network), which gets written back to RPC Result.

### Backend: NetworkManager

Use D-Bus via `zbus` (workspace already has tokio; `zbus` integrates cleanly).
Avoid shelling out to `nmcli`. Operations needed:

- Add a new `802-11-wireless` connection profile with the supplied SSID/PSK
  (`org.freedesktop.NetworkManager.Settings.AddConnection`).
- Activate it on the wifi device
  (`org.freedesktop.NetworkManager.ActivateConnection`).
- Wait for the active connection to reach `ACTIVATED` state, with a timeout
  (~30 s); on failure, delete the connection profile and return
  `Error::UnableToConnect`.
- For `scan`: trigger `RequestScan` on the wifi device, wait briefly, read
  `AccessPoints` and map flags → improv auth string (WEP / WPA / WPA2 / WPA3
  / WPA2 EAP / NO; multiples joined with `/`).
- For `get_hostname` / `set_hostname`: `org.freedesktop.hostname1`
  (`SetStaticHostname`).
- For `device_info`: `firmware = "bestool"`, `version = env!("CARGO_PKG_VERSION")`,
  `chip = std::env::consts::ARCH`, `device_name = hostname`. (Configurable via
  the backend's constructor so callers can override.)

Behind the `networkmanager` feature flag.

### BLE advertising

Use `bluer::adv::Advertisement` with:

- `service_uuids = { SERVICE_UUID }`
- `service_data = { 0x4677 → [current_state, capabilities, 0, 0, 0, 0] }`
- `discoverable = true`, `local_name = Some(device_name)`

Re-register the advertisement (or update via the manager handle) on every
state / capability change so the advertised state byte stays current. Bluer's
`adv_mon` API doesn't support live update, so cycle the registration handle.

### Multi-packet RPC reassembly

BlueZ delivers each `Write` as a separate callback. Buffer bytes in
`State::rpc_buffer: Mutex<Vec<u8>>`. After each append, attempt to parse a
complete packet from the front: header (cmd + length) + length bytes + 1
checksum byte. If the buffer doesn't yet hold `2 + length + 1` bytes, wait for
more. On parse error, clear the buffer and set `InvalidRPC`. (No timeout on
partial packets; caller-side resync is "send another command".)

### Authorization timeout

Spawn a tokio task on the `run` loop. On every `Authorized` transition (and
on `set_hostname` / `set_device_name` per spec), reset a `tokio::time::Sleep`.
On expiry, transition `Authorized` → `AuthorizationRequired`. The
`authorize_signal` watch from the configurator transitions back to `Authorized`.

### CLI integration

Add `bestool iti improv-wifi` (feature `iti-improv-wifi`):

```
bestool iti improv-wifi [--adapter hci0] [--device-name NAME] \
    [--no-authorize] [--auth-timeout 60s]
```

`--no-authorize` skips the physical-interaction gate (device starts
`Authorized`). For Tamanu Iti hardware with a button, we'll add an
`--authorize-gpio <pin>` flag in a follow-up — out of scope here, but the
trait accommodates it.

### Tests

- Unit tests for `rpc::parse` (round-trip every command, checksum verification,
  malformed inputs, multi-packet reassembly).
- Unit tests for `rpc::result` encoding (length prefixes, checksum, empty
  list, multi-string).
- Unit test for `Capabilities::byte()`.
- Integration: a fake `WifiConfigurator` impl + a mock GATT layer is too much
  yapping for v1. Skip; cover the protocol-level logic with unit tests and
  smoke-test the backend manually on a Pi.

## Implementation order

Each step is a commit. Stop at any point if blocked.

1. `plan: improv-wifi` — this file.
2. `chore(improv-wifi): add empty crate to workspace` — fresh `Cargo.toml`
   (edition 2024, current dep versions), empty `lib.rs`, register in workspace
   `members`. Confirms it builds.
3. `feat(improv-wifi): protocol constants and enums` — UUIDs, `Status`,
   `Error` (with `BadHostname`), `Capabilities` bitfield with `byte()` method.
   Unit tests for byte conversions.
4. `feat(improv-wifi): RPC packet parser` — winnow parser for command +
   length + data + checksum, including per-command payload parsers (Send Wi-Fi
   Settings ssid/password split, hostname/device-name set payloads). Unit tests.
5. `feat(improv-wifi): RPC result encoder` — encode a `Vec<String>` response
   into a packet with length prefixes and checksum. Unit tests.
6. `feat(improv-wifi): multi-packet write buffering` — small struct that
   accepts byte slices and yields complete `Rpc` values. Unit tests including
   split-mid-header, split-mid-payload, junk between packets.
7. `feat(improv-wifi): WifiConfigurator trait` — trait + `Capabilities` +
   `DeviceInfo` + `Network` types. No impls yet.
8. `feat(improv-wifi): GATT application` — port the bluer setup from the old
   branch, with all five characteristics correctly wired (read fns + notify
   loops for state/error, write+reassembly for RPC command, read+notify for
   RPC result). Pull in the `State` struct properly so notify loops actually
   fire.
9. `feat(improv-wifi): BLE advertising` — register `Advertisement` with
   service UUID + service data; re-register on state/capability change.
10. `feat(improv-wifi): authorization timeout` — timeout task driven by a
    watch channel.
11. `feat(improv-wifi): RPC dispatcher` — wire parsed commands to trait
    methods, encode responses to RPC Result, manage state transitions per
    spec (Authorized → Provisioning → Provisioned/Authorized on result).
12. `feat(improv-wifi): NetworkManager backend` — `zbus`-based impl behind
    feature flag. Provision happy path + connect-failure cleanup, scan,
    hostname get/set, device info.
13. `feat(bestool): iti improv-wifi command` — new subcommand wiring the
    crate up, behind `iti-improv-wifi` feature with `networkmanager` enabled.
14. `unplan: improv-wifi (all phases implemented)` — remove this plan.

## Out of scope

- iOS-style "Improv Serial" (USB/UART transport) — different protocol.
- Backends other than NetworkManager. `wpa_supplicant`/`iwd`/`networkd` can
  be added later; the trait is designed for it.
- GPIO-backed authorization on Tamanu Iti. Trait supports it; CLI flag is a
  follow-up.
- crates.io publication. Crate is structured for it but we won't push v0.1.0
  until the API has settled.
- macOS / Windows. The crate is `#[cfg(target_os = "linux")]`; bluer is
  Linux-only.

## Open questions

- Do we want the Improv service to **shut down** after `Provisioned`, per spec?
  Default yes (return from `run()` cleanly); user can re-launch if they need to
  reprovision. Confirm with user before implementing step 11.
- Authorization model for Tamanu Iti: button-press, time-window after boot, or
  no auth (start `Authorized`)? CLI v1 will offer `--no-authorize`; pick a
  default with the user.
