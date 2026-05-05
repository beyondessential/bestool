# Plan: STDIN + GPIO authorization triggers for `bestool iti improv-wifi`

## Why

The improv-wifi command supports `--require-authorization` (start in
`AuthorizationRequired`, gate credential writes), but there's no way to
move from `AuthorizationRequired` to `Authorized`. This plan adds two
trigger sources:

- **STDIN:** any line read from stdin authorizes the session. Lets a
  service runner (systemd, ssh shell, helper script) authorize without
  hardware.
- **GPIO button press:** a wired button on a configurable BCM pin
  authorizes. The expected deployment.

Each authorization is one-shot per timeout window: the existing
`auth_timeout` reverts state back to `AuthorizationRequired` if no
provisioning attempt happens. Re-pressing or re-sending stdin re-authorizes.

## Scope

In scope:

- New public `AuthHandle` in `improv-wifi`: cloneable, `authorize()`
  signals the running service to set `Status::Authorized`.
- `ImprovWifi::auth_handle()` returns one. `install`/`run` plumbing
  listens on the channel and calls `state.set_status(Authorized)`.
- New CLI flags on `bestool iti improv-wifi`:
  - `--auth-stdin`
  - `--auth-gpio <BCM-pin>`
  - Either implies `--require-authorization`.
- Pull `rppal` into the `iti-improv-wifi` feature (already used by
  `iti-battery` / `iti-lcd`, so dep is in the workspace).

Out of scope:

- Multiple GPIOs / chord support / LED indicator. Single button only.
- Configurable pin polarity beyond the safe default (active-low,
  internal pull-up — i.e. button shorts the pin to GND when pressed).
  Add a flag later if a deployment needs the inverse.
- Refactoring the existing `--require-authorization` flag name. Keep it,
  document that the new flags imply it.

## Library API change

```rust
// New public type, non-generic so callers don't pay the T tax.
#[derive(Clone)]
pub struct AuthHandle { /* mpsc::UnboundedSender<()> inside */ }

impl AuthHandle {
    /// Signal the service to enter Authorized.
    pub fn authorize(&self);
}

impl<T: WifiConfigurator + 'static> ImprovWifi<T> {
    pub fn auth_handle(&self) -> AuthHandle;
}
```

Wiring: `install` creates an `mpsc::unbounded_channel::<()>()`. The
sender lives in the `AuthHandle`. The receiver is owned by the run loop
and `tokio::select!`'d alongside the existing status-change /
provisioned receivers; on a `()` it calls `state.set_status(Authorized)`.

The existing `ImprovWifi::authorize(&self)` stays — it's the same
operation, just available on the value before `run` consumes it.

## CLI

```rust
/// Authorize when a line is received on stdin (any line, content
/// ignored). Implies --require-authorization.
#[arg(long)]
pub auth_stdin: bool,

/// Authorize on a button press on this BCM GPIO pin (active-low,
/// internal pull-up). Implies --require-authorization.
#[arg(long)]
pub auth_gpio: Option<u8>,
```

Implementation:

1. If either flag is set, force `AuthorizeMode::Required`.
2. Build the service, get the `AuthHandle`.
3. If `--auth-stdin`: spawn a tokio task using `tokio::io::stdin()` +
   `BufReader::lines()`, calling `auth.authorize()` on each line read.
4. If `--auth-gpio <pin>`: open `rppal::gpio::Gpio`, take the pin as
   input with pull-up, register an async interrupt on
   `Trigger::FallingEdge` with debounce. The callback signals the handle.
5. `service.run().await` drives the BLE side. On exit, abort the stdin
   task; the GPIO pin guard drops to clear the interrupt.

## Phases

1. **Library: AuthHandle type + plumbing.** `AuthHandle` lives near
   the service module. `install` creates the channel, stashes the
   receiver in `AppHandles`, exposes the sender via
   `ImprovWifi::auth_handle()`. `run`'s `select!` wakes on the channel
   and calls `set_status(Authorized)`.

2. **CLI flags + wiring.** Add the two args, force-Required logic,
   stdin task, GPIO task.

3. **Cargo.toml.** Add `rppal` to `iti-improv-wifi` feature.

4. **USAGE.md.** Run `./update-usage.sh` to refresh the help output.

## Testing

- `cargo clippy --all-targets --all-features` and `cargo fmt`.
- Unit test for `AuthHandle::authorize()` triggering status transition
  through the receiver-task path (use a fake `WifiConfigurator`).
- Manual smoke test (deferred, can't be done from sandbox):
  - `bestool iti improv-wifi --require-authorization --auth-stdin`,
    verify pressing Enter logs the auth and the status characteristic
    transitions.
  - `bestool iti improv-wifi --auth-gpio 17` on a Pi with a button
    wired GPIO17 → GND, verify press authorizes.

## Risks / unknowns

- `tokio::io::stdin()` is line-buffered on TTYs but not necessarily on
  pipes. `BufReader::lines` handles both. EOF on stdin terminates the
  task without taking the service down — that's correct.
- rppal's async interrupt fires on a dedicated thread; the callback
  must be cheap. `UnboundedSender::send` is non-blocking and cheap.
- If both `--auth-stdin` and `--auth-gpio` are set, both run; either
  triggers authorize.
