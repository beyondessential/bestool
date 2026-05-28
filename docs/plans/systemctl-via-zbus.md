# Replace systemctl subprocess calls with zbus_systemd

## Context

Every interaction the tamanu commands have with systemd is currently a subprocess call to `systemctl`. There are nine such call sites across `crates/bestool/src/actions/tamanu/` and `crates/tamanu/src/`, all gated by `cfg!(target_os = "linux")` (Windows uses pm2). Two parse text output (`list-units` and `is-enabled`); the rest only check exit codes.

`zbus_systemd` (0.26000.0, pure-Rust D-Bus, MIT/Apache, regenerated from systemd's introspection XML) gives us typed access to the same operations: structured results from `ListUnitsByPatterns`, typed errors, `JobRemoved` signals for deterministic completion waits, no fork-per-poll. `zbus` 5.15 is already a workspace dep via `improv-wifi`, so no new C dep tree.

## Call sites to migrate

| Site | Current command |
|------|-----------------|
| `crates/bestool/src/actions/tamanu/lifecycle.rs:130` `discover_systemd` | `systemctl list-units --type=service --all --no-legend --plain --no-pager tamanu-*.service` |
| `crates/bestool/src/actions/tamanu/lifecycle.rs:265` `stop_targets` | `systemctl stop <units>` |
| `crates/bestool/src/actions/tamanu/lifecycle.rs:342` `disable_systemd_units` | `systemctl disable <units>` |
| `crates/bestool/src/actions/tamanu/lifecycle.rs:388` `restart_one` | `systemctl restart <unit>` |
| `crates/bestool/src/actions/tamanu/lifecycle.rs:466` `reload_caddy` | `systemctl reload caddy` |
| `crates/bestool/src/actions/tamanu/lifecycle.rs:621` `is_running` | `systemctl is-active --quiet <unit>` |
| `crates/bestool/src/actions/tamanu/start.rs:265` `systemctl_start` | `systemctl start <units>` |
| `crates/bestool/src/actions/tamanu/restart.rs:197` `bulk_restart` | `systemctl restart <units>` |
| `crates/tamanu/src/services.rs:327` `systemd_is_enabled` | `systemctl is-enabled <unit>` |
| `crates/tamanu/src/doctor/checks/tamanu_service.rs:124` `discover_systemd` | same as lifecycle's |

Out of scope: `resolvectl flush-caches` (separate `org.freedesktop.resolve1` interface), `sudo` re-exec in `ensure_root_or_reexec` (sudo is correct), and journal reading (see `journal-reader.md`).

## Implementation

### Wrapper module

New module `crates/tamanu/src/systemd.rs`, Linux-only (`#[cfg(target_os = "linux")]`). Exposes:

```rust
pub async fn list_tamanu_units() -> Result<Vec<UnitState>>;
pub async fn start(units: &[&str]) -> Result<()>;
pub async fn stop(units: &[&str]) -> Result<()>;
pub async fn restart(unit: &str) -> Result<()>;
pub async fn reload(unit: &str) -> Result<()>;
pub async fn disable(units: &[&str]) -> Result<()>;
pub async fn is_active(unit: &str) -> Result<bool>;
pub async fn is_enabled(unit: &str) -> Result<bool>;
```

`UnitState { name, load_state, active_state, sub_state }` — fields match what the current text parser extracts so call sites translate cleanly.

`restart` subscribes to `JobRemoved` before issuing `RestartUnit`, then awaits the matching job-removed signal. This replaces the `wait_running` polling for the rolling-restart path in `restart_one`. Bulk operations (`start`, `stop`, `disable`) use mode `"replace"` and don't wait — callers poll via `wait_running`/`wait_stopped` as today (which now call `is_active` per unit).

Connection caching: a `tokio::sync::OnceCell<zbus::Connection>` so we open one `Connection::system().await?` per process. Reset on connect failure isn't worth handling — if the bus drops we have bigger problems.

### Cargo changes

In `crates/tamanu/Cargo.toml`:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
zbus_systemd = { version = "0.26000.0", default-features = false, features = ["systemd1", "zbus-async-tokio"] }
```

Behind a target cfg so Windows/macOS builds don't pull it. The wrapper module is also `#[cfg(target_os = "linux")]`; non-Linux builds that try to call into it get compile errors at the call site (currently call sites are themselves gated, so this is consistent).

### Call site migration

One commit per site (or per coherent group). Order:

1. `services.rs::systemd_is_enabled` → `systemd::is_enabled` (smallest, smoke-test for the wrapper)
2. `lifecycle.rs::is_running` → `systemd::is_active`
3. `lifecycle.rs::discover_systemd` + doctor's mirror → `systemd::list_tamanu_units`
4. `lifecycle.rs::stop_targets`, `start.rs::systemctl_start`, `lifecycle.rs::disable_systemd_units` → wrapper calls
5. `lifecycle.rs::restart_one` (with JobRemoved) and `restart.rs::bulk_restart` → wrapper calls
6. `lifecycle.rs::reload_caddy` → `systemd::reload`

Each commit propagates `async` upward as needed. Tamanu subcommands are already inside `tokio::main`, so this is plumbing, not architectural change.

## Verification

- `cargo check --target x86_64-pc-windows-gnu` — confirms zbus_systemd is gated out
- `cargo check --target x86_64-unknown-linux-gnu`
- `cargo clippy && cargo fmt`
- Existing lifecycle tests pass
- On a live tamanu host:
  - `bestool tamanu status` — same output as before
  - `bestool tamanu stop && bestool tamanu start` — succeeds; services come back up
  - `bestool tamanu restart` — rolling restart still observes per-instance health
  - `bestool tamanu doctor` — service section reports the same
