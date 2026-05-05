# Plan: port improv-wifi BLE peripheral from `bluer` to `zbus`

## Why

The improv-wifi crate currently uses `bluer` for the BLE peripheral / GATT
server. `bluer` depends on the C-FFI `dbus` crate (→ `libdbus-sys`), which
breaks our `aarch64-unknown-linux-musl` static build (the iti deployment
target) and forces every Linux CI job to install `libdbus-1-dev`.

We already use `zbus` (pure-Rust D-Bus) for the NetworkManager backend.
Switching the BLE side to zbus removes the C dep, lets the iti build stay
musl-static, and unifies the D-Bus stack inside the crate.

There is no off-the-shelf zbus-based BlueZ GATT-server crate that's
maintained, published, and works with zbus 5. We implement the BlueZ D-Bus
surface ourselves.

## Scope

In scope:

- Replace `bluer` usage in `crates/improv-wifi/src/{gatt,service,lib}.rs`.
- Replace `bluer::Session`/`Adapter` setup in
  `crates/bestool/src/actions/iti/improv_wifi.rs`.
- Drop `bluer` from both crates' `Cargo.toml`.
- Update CI to no longer need `libdbus-1-dev` for the iti build (and
  generally drop it where it was only needed for `improv-wifi`/`bluer`).

Out of scope:

- Protocol changes. The RPC parser, reassembler, state machine, error
  enum, `WifiConfigurator` trait, and `NetworkManagerBackend` are
  unchanged.
- Improv-Wifi UUIDs and behaviour. Same advertisement, same
  characteristics, same flow.
- New iti subcommands or device-side features.

## Surface to implement on top of zbus

Object tree (paths illustrative):

```
/au/bes/improv               (ObjectManager root)
  /au/bes/improv/service0    (org.bluez.GattService1)
    /au/bes/improv/service0/char0..4
                             (org.bluez.GattCharacteristic1)
/au/bes/improv/adv0          (org.bluez.LEAdvertisement1)
```

Interfaces we own (zbus `#[interface]` impls):

- `org.bluez.GattService1` — read-only properties: `UUID`, `Primary`.
- `org.bluez.GattCharacteristic1` — properties: `UUID`, `Service`,
  `Flags`, `Value`, `Notifying`. Methods: `ReadValue(options)`,
  `WriteValue(value, options)`, `StartNotify`, `StopNotify`. Notify
  emits `PropertiesChanged` on `Value`.
- `org.bluez.LEAdvertisement1` — properties: `Type`, `ServiceUUIDs`,
  `ServiceData`, `LocalName`, `Discoverable`. Method: `Release`.
- `org.freedesktop.DBus.ObjectManager` — zbus has built-in support
  (`ObjectServer::at` + `Connection::object_server()` exposes
  `GetManagedObjects` if we register the path correctly; if not, we
  implement it manually).

Calls we make (zbus proxies on `org.bluez`):

- `org.bluez.GattManager1.RegisterApplication(object, dict)` /
  `UnregisterApplication(object)` on the adapter path.
- `org.bluez.LEAdvertisingManager1.RegisterAdvertisement(object, dict)` /
  `UnregisterAdvertisement(object)` on the adapter path.
- `org.bluez.Adapter1` properties — we need `Powered` (set true) and the
  ability to find the default adapter (iterate ObjectManager on
  `org.bluez` and pick the first object with an `Adapter1` interface,
  or honour `--adapter <hciN>`).

## Phases

1. **Cargo plumbing.** Make `zbus` a non-optional dep of `improv-wifi`
   on Linux; gate only the NetworkManager backend module behind the
   `networkmanager` feature, not zbus itself. Add `uuid = "1"` directly
   (don't rely on the bluer re-export). Don't drop `bluer` yet — keep
   the crate compiling as we go.

2. **BlueZ proxy types.** New `crates/improv-wifi/src/bluez/proxy.rs`
   with `#[zbus::proxy]` definitions for `Adapter1`, `GattManager1`,
   `LEAdvertisingManager1`. Adapter discovery helper that walks
   ObjectManager on `org.bluez` to find the first powered adapter or a
   named one.

3. **Advertisement object.** New `bluez/advertisement.rs` implementing
   `LEAdvertisement1` as a zbus `#[interface]`. Holds the values from
   the existing `build_advertisement` helper. `Release` shuts down our
   side cleanly.

4. **GATT objects.** New `bluez/gatt.rs` with one `#[interface]` per
   characteristic plus a service interface. Each characteristic owns an
   `Arc<State<T>>` and implements `ReadValue` / `WriteValue` /
   `StartNotify` / `StopNotify` directly, replacing the current
   closure-based `Characteristic` builders. Notify-capable
   characteristics drive `PropertiesChanged` signals from the existing
   `broadcast::Sender<...>` channels in `State`.

5. **Application lifecycle.** Replace `serve_gatt_application` and
   `adapter.advertise(...)` with: open zbus system bus connection,
   register objects on the object server, call `RegisterApplication`
   and `RegisterAdvertisement`, await the run loop, then unregister and
   drop. The existing `run()` state machine (status changes, auth
   timeout, provisioned shutdown) stays — it just calls
   re-register-advertisement instead of `adapter.advertise` for the
   service-data update on status change.

6. **bestool iti command.** Replace `bluer::Session::new` /
   `default_adapter` / `set_powered(true)` with zbus equivalents using
   the new `bluez::proxy::Adapter1Proxy`.

7. **Drop `bluer`.** Remove the dep from both `Cargo.toml`s. Drop the
   `bluer::Uuid` re-export — switch all UUID constants to `uuid::Uuid`.

8. **CI.** Remove the temporary `libdbus-1-dev` install (if we add one
   as a stopgap — we won't, since this plan replaces bluer in one go).
   Verify all Linux CI jobs (clippy, tests on x86 + arm, all build
   targets including aarch64-unknown-linux-musl with `--features iti`)
   are green.

## Testing

- `cargo clippy --all-targets --all-features` and `cargo fmt`.
- `cargo test --workspace` (existing protocol/state tests still run).
- Manual BLE smoke test on a real Linux+BlueZ host: scan from a phone
  using the Improv-Wifi web client, verify advertisement, GATT
  read/write/notify, and successful provisioning. Document the manual
  steps in commit message — I can't BLE-test from CI.

## Risks / unknowns

- zbus 5 ObjectManager export ergonomics. If the built-in support
  doesn't cover what BlueZ expects from `GetManagedObjects`, we
  implement the interface by hand on the root object — straightforward
  but a bit of code.
- BlueZ's `RegisterApplication` is picky about the dict layout it walks
  via ObjectManager. The interfaces it expects are documented but the
  exact property names and types must match exactly (`Vec<String>` vs
  array of `OwnedValue` for `Flags`, etc.). Reference the BlueZ
  `gatt-api.txt` spec while implementing.
- Notify behaviour: we currently spawn a task per `StartNotify` call.
  With zbus `#[interface]`, `StartNotify` should set a flag and the
  push side emits `PropertiesChanged` via the object-server signal API.
  The existing broadcast channels in `State` already feed this — we
  swap the consumer.
