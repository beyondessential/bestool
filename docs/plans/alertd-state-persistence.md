# alertd state persistence

## Problem

`AlertState.triggered_at` (and `last_sent_at`, `last_output`, `paused_until`)
lives only in memory. When the daemon restarts — upgrade, crash, host
reboot — the new process starts with `triggered_at = None`.

If an alert was active at the moment of restart and the underlying condition
later clears, the daemon sees `was_triggered = false` and emits no
`send_clear`. Canopy stays stuck on `active=true` indefinitely.

The recent retry-on-failure fix
(`fix(alertd): retry canopy send_clear on failure ...`) closes the transient
network failure case but does nothing for restart amnesia.

## Approach

Persist `AlertState` to a single JSON file. Load on startup. Save after every
state change, debounced.

## Design

### File path — fixed, cross-platform default

No CLI flag. Path is derived from the `dirs` crate, same pattern as
`bestool-psql`'s audit DB:

- Linux: `dirs::state_dir()` → `~/.local/state/bestool-alertd/state.json`,
  with XDG/HOME fallbacks.
- macOS/Windows: `dirs::data_local_dir()` →
  `~/Library/Application Support/bestool-alertd/state.json` /
  `%LOCALAPPDATA%\bestool-alertd\state.json`, with hardcoded fallbacks.

Create the parent directory on startup if missing. If we can't establish a
path at all, log a warning and run without persistence (the alert loop keeps
working — persistence is best-effort, not a hard dependency).

### Format

```json
{
  "saved_at": "2026-05-13T15:00:00Z",
  "alerts": {
    "/etc/tamanu/alerts/disk-full.yml": {
      "triggered_at": "2026-05-13T14:55:00Z",
      "last_sent_at": "2026-05-13T14:55:00Z",
      "last_output": "...",
      "paused_until": null
    }
  }
}
```

- Alert keys are the canonical `AlertDefinition.file` paths.
- No schema version field. If the format ever changes, deal with it then.
- Unknown alerts (file path no longer in loaded set) → dropped on next save.

### Atomic writes

`tempfile::NamedTempFile::new_in(parent)` + `persist()` to the final
filename. Same filesystem so rename is atomic.

### Save strategy — dedicated task + Notify, debounced

1. Add `state_dirty: Arc<tokio::sync::Notify>` to `Scheduler`.
2. Every site in `scheduler.rs` that writes to `state.triggered_at`,
   `state.last_sent_at`, `state.last_output`, or `state.paused_until` calls
   `state_dirty.notify_one()` after releasing the lock.
3. A `persistence_task` loops:
   - `state_dirty.notified().await`
   - `sleep(500ms)` to coalesce bursts
   - snapshot via `scheduler.get_alert_states()`
   - write tempfile + atomic rename
   - on error: log, retry on next notify (don't block ticks)

### Load on startup

Hydrate per-alert inside `load_and_schedule_alerts`, between creating each
`AlertState` and spawning its task — otherwise a task could tick and emit a
clear before hydration runs.

The flow:

1. Read + parse the file. If missing → empty map (first run, expected).
2. If unreadable or unparseable → log warning, **delete the file** and use
   empty map. Next save replaces it with a clean copy.
3. For each alert path in the loaded scheduler, look up the entry by
   canonical path and seed `triggered_at`, `last_sent_at`, `last_output`,
   `paused_until`. Orphans (entries with no matching loaded alert) are
   ignored and dropped at the next save.

### Failure modes

| Situation | Behaviour |
| --- | --- |
| File missing | Empty map. First save creates it. |
| File unreadable / unparseable | Log warn, delete, empty map. |
| Save fails (disk full, permission) | Log error, daemon keeps running, retry on next notify. |
| Can't even determine a path | Log warn, persistence disabled, daemon keeps running. |

Persistence is best-effort. The daemon must never fail to start or crash
because of a state-file problem.

### Reload interop

`preserve_state_from` already handles hot config reload. The persistence
layer leaves that alone — it only hydrates at cold start, and saves
whenever in-memory state changes (which includes the post-reload state,
naturally).

## Implementation

Files to touch:

- `crates/alertd/Cargo.toml` — add `dirs` dependency.
- `crates/alertd/src/state_file.rs` — new module: path resolution, serde
  types, read/write/atomic-rename, delete-on-corruption.
- `crates/alertd/src/lib.rs` — `pub mod state_file;`.
- `crates/alertd/src/scheduler.rs`:
  - new `state_dirty: Arc<Notify>` field on `Scheduler`.
  - `notify_one()` calls after each state mutation.
  - `hydrate_from_state_file(map)` invoked inside
    `load_and_schedule_alerts` between state creation and task spawn.
- `crates/alertd/src/daemon.rs`:
  - resolve state file path at startup, ensure parent dir exists.
  - read state file before `scheduler.load_and_schedule_alerts()` and pass
    the parsed map in.
  - spawn the persistence task wired to `state_dirty`.

Tests:

- Unit: path resolution returns Some on the test host.
- Unit: `read_state_file` on missing path → Ok(empty).
- Unit: `read_state_file` on corrupt file → Ok(empty) **and** the file is
  deleted.
- Unit: round-trip serialise/deserialise preserves all four fields.
- Unit: atomic write (write twice, second write replaces first).
- Integration: scheduler hydrate seeds `triggered_at` correctly for matched
  alerts and skips entries for alerts not in the loaded set.

No new dependencies beyond `dirs` (already used by `bestool-psql`).

## Out of scope (explicit)

- Cross-host state.
- Canopy-side reconciliation (probing canopy on startup for stuck refs).
- Configurable path / CLI flag for the state file — the `dirs` default is
  the only path.
- Schema version field. If we ever change the format, handle it then.
