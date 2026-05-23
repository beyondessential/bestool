# improv-wifi

A Rust implementation of the [Improv Wi-Fi] BLE peripheral protocol — the
*device* side of the provisioning conversation. Use it on a headless Linux box
to advertise itself over Bluetooth Low Energy so a mobile app or
[improv-wifi.com](https://www.improv-wifi.com) in the browser can hand over
Wi-Fi credentials without a display or keyboard.

[Improv Wi-Fi]: https://www.improv-wifi.com

## Scope

Currently this crate covers:

- **BLE transport, via BlueZ on Linux.** The GATT service, advertisement, RPC
  reassembly, and the full state machine (authorisation → provisioning →
  provisioned) are implemented on top of zbus.
- **A `WifiConfigurator` trait** that decouples the protocol from how the
  device actually talks to its network stack. Implement it yourself, or enable
  the `networkmanager` feature for a built-in backend that drives Wi-Fi via
  NetworkManager's D-Bus API.

## Use

```toml
[dependencies]
improv-wifi = { version = "0.1", features = ["networkmanager"] }
```

The high-level flow:

1. Connect to the system bus and find a BlueZ adapter with `find_adapter` /
   `power_on_adapter`.
2. Construct an `ImprovWifi` with your `WifiConfigurator` and an
   `ImprovWifiConfig` (authorisation mode, advertised local name, etc.).
3. Call `ImprovWifi::run` to drive advertising and the state machine until
   the device is provisioned.

If your device gates provisioning on a physical button press, hold an
`AuthHandle` from another task and call `authorize()` when the user interacts.

See the crate-level rustdoc for the full API.

## Not yet implemented, but we'd take patches

The Improv-Wi-Fi spec describes more than just the BLE transport on Linux, and
this crate is happy to grow:

- **Serial transport.** Improv-Wi-Fi defines a USB/UART variant of the same
  RPC; useful for microcontrollers and for devices that already expose a
  serial console.
- **Embedded / `no_std` targets.** The protocol itself (RPC framing,
  reassembly, state machine) does not need an allocator or BlueZ; it would
  be useful as a transport-agnostic core that an ESP32 or similar could
  drive directly.
- **Other Linux backends.** A direct `wpa_supplicant` or `iwd` backend, for
  systems that don't run NetworkManager.
- **Other host operating systems.** macOS or Windows BLE peripherals if
  someone has a use case.

If any of these would be useful to you, open an issue or PR.

## License

GPL-3.0-or-later.
