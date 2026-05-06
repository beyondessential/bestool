# Plan: long-running `iti improv-wifi --watch` mode

## Why

Today the iti improv-wifi service is a one-shot:
- Boot, exit if Wi-Fi is connected, otherwise advertise + provision once.

The user wants a daemon mode that:

1. **Stays alive.** Single long-running process.
2. **Doesn't advertise when Wi-Fi is configured.** "Secure at rest": a
   provisioned device is not visible over BLE.
3. **Long-press (3 s+) on the auth-GPIO button starts advertising —
   even if Wi-Fi is currently connected.** Lets a person on-site re-
   provision (e.g. for a network migration) without first nuking the
   existing wifi config.
4. **Advertises immediately at startup if no Wi-Fi config exists.** The
   first-boot / initialisation workflow still works without anyone
   present.

So the GPIO button gains a second meaning: short press = authorise
(during advertising), long press = enter advertising (during idle).

## Scope

In scope:

- `NetworkManagerBackend::is_configured()` — true iff any saved
  `802-11-wireless` connection profile exists.
- `bestool iti improv-wifi --watch` flag. Implies and requires
  `--auth-gpio`. Conflicts with `--auth-stdin` (stdin triggers would
  defeat the physical-presence guarantee).
- New flag `--auth-gpio-long-press <DUR>` (default `3s`) — minimum hold
  for a long press.
- Watcher loop in the iti command:
  - Detects short-vs-long press by recording press timestamp on the
    falling edge and classifying on the rising edge.
  - At startup, if `!is_configured` ⇒ run an advertising session
    immediately. Otherwise idle.
  - In idle: wait for long-press → run an advertising session → idle.
  - In advertising session: install service, route short presses to the
    `AuthHandle`, run until `service.run` returns (provisioned or BLE
    error), then return to idle.
- Switch the systemd unit file to `--watch`.

Out of scope:

- A library-level "watcher" abstraction. The orchestration lives in
  bestool — the lib stays focused on the protocol + BLE.
- Idle-timeout for advertising sessions. A session runs until
  provisioned or until the process is killed. Long-press while a
  session is running is ignored (no-op).
- LED indicator for state. Possibly a follow-up.
- Stdin participation in `--watch` mode. Disallowed by clap.

## Library changes

`crates/improv-wifi/src/networkmanager.rs`:

- New proxies for `org.freedesktop.NetworkManager.Settings` (with
  `ListConnections`) and extending the existing
  `NmSettingsConnection` proxy with `GetSettings()`.
- New `pub async fn is_configured(&self) -> Result<bool, Error>` on
  `NetworkManagerBackend`. Iterates list of saved connections, returns
  `true` on the first whose `connection.type` is `"802-11-wireless"`.

The library doesn't grow a watcher type — the orchestration belongs to
the deployment.

## CLI: bestool iti improv-wifi

New flags:

```rust
/// Stay alive after Wi-Fi is provisioned. Advertising is gated by:
/// - Wi-Fi *not* configured at startup (initialisation), or
/// - a long press on --auth-gpio (re-provisioning).
///
/// Implies --auth-gpio.
#[arg(long, requires = "auth_gpio", conflicts_with = "auth_stdin")]
pub watch: bool,

/// Hold time on --auth-gpio that counts as a long press.
#[arg(long, default_value = "3s")]
pub auth_gpio_long_press: humantime::Duration,
```

The existing `--auth-gpio` still configures a single-press authoriser
in non-watch mode.

In `--watch` mode, GPIO handling is replaced with a press classifier:

1. `set_async_interrupt(Trigger::Both, debounce, ..)` sends both edges
   to a tokio channel.
2. A tokio task records `Instant` on the falling edge (button down,
   pull-up active) and classifies on the rising edge (button up):
   - `elapsed >= long_press_window` → send on `long_press_tx`.
   - `elapsed < long_press_window` → send on `short_press_tx`.
3. Outer watcher loop (`select!` on the right channel based on state):
   - **Idle:** await `long_press_rx`. On signal → advertising.
   - **Advertising:** install service, spawn a task that forwards
     `short_press_rx` → `auth_handle.authorize()`, then
     `service.run().await`. After it returns, drop everything and go
     back to idle.

Initial state: `if !is_configured { advertise } else { idle }`.

## Service file

```
ExecStart=/usr/local/bin/bestool --log-timeless iti improv-wifi --watch --auth-gpio 17
Restart=always
RestartSec=10s
```

`Restart=always` because the service should always be present —
crashes get respawned, and a clean exit (which only happens on
unexpected protocol errors in watch mode) also gets respawned.

## Phases

1. **Library: `is_configured`.** Add NM Settings proxies and the
   method. Test build.

2. **CLI: long-press classifier.** Replace the simple
   `set_async_interrupt(FallingEdge, ...)` with a `Both`-edge classifier
   that emits to two channels. Wire it behind `--watch`.

3. **CLI: watcher loop.** State machine over (idle, advertising) with
   the classification channels. Initial branch on `is_configured`.

4. **Service file.** Switch ExecStart, switch Restart policy.

5. **USAGE.md.** Refresh.

## Testing

- `cargo clippy --all-targets --all-features` and `cargo fmt`.
- `cargo test --workspace`.
- Manual smoke test (deferred — needs hardware):
  - Fresh device with no wifi profile: `--watch` advertises immediately.
  - After provisioning: subsequent restarts stay idle.
  - Long press on the button: advertising starts.
  - Short press during advertising: authorises.
  - Long press while idle, then short press during the resulting
    session: authorises.
  - Power-cycle while idle: stays idle.

## Risks / unknowns

- Press classification skipping edges. rppal's debounce should
  collapse contact bounce. If a user's button is gnarly we may also
  need a software floor on the press duration (e.g. ignore < 50 ms).
  Default debounce of 50 ms is the existing setting and should be
  enough.
- `is_configured` semantics. Currently uses
  `Settings.ListConnections` + `connection.type == "802-11-wireless"`.
  Could miss a connection profile gated behind unusual settings. If
  this proves too coarse we can switch to checking the device's
  `AvailableConnections` property.
- If a long press is held continuously (user doesn't release the
  button), the rising edge never comes. We never enter advertising.
  That's fine — it's an unambiguous "user held button" signal that
  matches the "release to confirm" UX of typical buttons.
