# alertd: clear canopy events for internal events

## Problem

`EventManager::trigger_event` only ever sends `active:true` to canopy. There is
no `trigger_clear` counterpart, so internal events (`database-down`,
`source-error`, `definition-error`) trigger a canopy issue and then never
clear it when the underlying condition recovers.

Three observable symptoms:

1. **database-down**: `daemon.rs` flips `was_down = true` and fires
   `EventType::DatabaseDown`. When the database recovers, `was_down` is
   flipped back to `false` and a `"database connection restored"` line is
   logged — but no clear event is sent.
2. **source-error**: a SQL alert whose `read_sources` returns `Err` fires
   `EventType::SourceError` on every tick that errors. The SQL alert's own
   `send_clear` runs once the SQL state recovers, but the source-error event
   itself is never cleared.
3. **definition-error**: every reload fires `EventType::DefinitionError` for
   each broken file. A file that recovers on the next reload never produces
   a clear.

Secondary issue: when the default-target path is used (no explicit alert
configured for the event), the synthetic alert's file is `[internal:<event>]`,
so the canopy ref collides across all firings on the same host. To clear per
entity, the synthetic file must embed the entity key (the erroring alert's
file, the broken file path, etc.) so trigger and clear share a ref.

## Approach

### 1. `EventManager::trigger_clear`

Add a sibling to `trigger_event` on `EventManager` that:

- Looks up explicit alerts for the event type and calls `target.send_clear`
  on each resolved target.
- Falls back to the default target if no explicit alert is configured,
  synthesising the same alert struct that `trigger_event` would synthesise
  (so canopy refs match).

### 2. Entity key threading

Add an optional `entity_key: Option<&str>` parameter to both `trigger_event`
and `trigger_clear`. When `Some`, embed it in the synthetic alert's file so
the canopy ref is unique per entity:

- `DatabaseDown`: `None` (one database per daemon).
- `SourceError`: `Some(alert.file)` — each erroring alert gets its own
  canopy issue and can clear independently.
- `DefinitionError`: `Some(file)` — same.
- `Http`: `None` (fire-and-forget, no clear).

Synthesised file format: `[internal:<event>]` or
`[internal:<event>:<entity_key>]`. The colon-separated stem stays stable
across trigger and clear so canopy dedups correctly.

### 3. Recovery state

- **DatabaseDown**: already tracked via `was_down` in `daemon.rs`. On the
  transition `was_down=true → healthy`, call `trigger_clear(DatabaseDown,
  None)`.
- **SourceError**: add `source_was_erroring: bool` to `AlertState` and
  `PersistedAlertState`. In the alert task tick, on transition
  `source_was_erroring=true → read_sources OK`, call
  `trigger_clear(SourceError, Some(file))`.
- **DefinitionError**: track the set of files that errored during the last
  load in a field on `Scheduler` (no persistence needed — on cold start any
  previously erroring files will either still error or load cleanly, and a
  spurious clear for a never-triggered file is harmless because canopy
  refs are deterministic). On the next load, for any file that was in the
  previous set but is not in the new `definition_errors` list, call
  `trigger_clear(DefinitionError, Some(file))`.

### 4. Tests

- Unit test that `trigger_clear` invokes `send_clear` rather than `send` on
  resolved targets (use a dry-run target or mock).
- Unit test that synthesised file embeds entity_key.
- Integration-ish test (in scheduler.rs) for `source_was_erroring`
  transitions if feasible without too much scaffolding.

## Out of scope

- Clearing for HTTP events (no recovery semantics).
- Backfilling existing canopy issues for events that have already been
  triggered before this change ships — operators can clear manually.

## Files touched

- `crates/alertd/src/events.rs` (add `trigger_clear`, thread `entity_key`)
- `crates/alertd/src/daemon.rs` (call `trigger_clear` on db recovery)
- `crates/alertd/src/scheduler.rs` (track source/def error state, call clears)
- `crates/alertd/src/state_file.rs` (persist `source_was_erroring`)
