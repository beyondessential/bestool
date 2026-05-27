# `bestool tamanu sync` — trigger and watch a manual sync

## Problem

Operators on a facility server occasionally want to kick a sync
immediately rather than wait for the cron-scheduled one (`*/1 * * * *`
by default, but commonly delayed when the previous run is still going
or when the central is queueing the device). Today the options are:

- Open the desktop client, log in as a real user, and press the "Sync
  now" button — which hits the authed `/api/sync/run` route with a 10s
  timeout, so the response is rarely the actual sync result.
- SSH in and `curl -X POST http://localhost:4100/sync/run` against the
  facility-server sync sub-process — which works, but the operator now
  has to also tail `journalctl -fu tamanu-facility-sync` in another
  pane to see what's happening, and remember to stop watching when the
  POST returns.

bestool already knows the supervisor model (systemd vs pm2), how to
find the install root, and how to tail service logs (`tamanu logs`).
A first-class `tamanu sync` command is the obvious shape.

## Goals

- One command that triggers a manual sync against the running facility
  sync sub-process and shows the operator what the sync service is
  doing while it runs.
- Works on both systemd (Linux) and pm2 (Windows) facility deployments.
- Exits with a non-zero status if the sync did not run successfully
  (disabled, error from the trigger endpoint, sync not enabled).
- Reasonable behaviour if invoked on a central server (clear bail with
  context, not a confusing localhost connection error).

## Non-goals

- Auto-restarting the sync sub-process. If it's not up,
  `tamanu status` / `tamanu restart tamanu-facility-sync` are the
  right tools.
- Reimplementing sync logic locally. We just trigger the existing
  endpoint and watch.
- Cancelling an in-flight sync. The sync API has no cancel route.
- A `--quiet` / scripting mode that suppresses logs. Easy follow-up if
  needed, but the headline use is interactive.

## CLI shape

```
bestool tamanu sync [--no-follow] [--lines N] [--timeout DURATION]
```

- `--no-follow` — fire the POST, print the result, exit. No log
  tailing.
- `--lines N` (default 10) — number of trailing log lines to print
  before tailing begins. Mirrors `tamanu logs -n`.
- `--timeout DURATION` (default unset) — give up after this long even
  if `/sync/run` hasn't returned. Without a timeout the command waits
  indefinitely, which matches the sync sub-process behaviour and is
  the right default for the interactive use case.

No positional arguments. Aliases: none — the subcommand is short
already.

## Architecture

### Sync triggering API

The facility-server sync sub-process exposes two non-authed routes,
mounted on a separate Express app bound to
`config.sync.syncApiConnection.host` + `port` (default
`http://localhost:4100`) — see
`tamanu/packages/facility-server/app/routes/sync/sync.js` and
`createSyncApp.js`.

- `POST /sync/run` — body `{ syncData: { type, urgent, ... } }`.
  Blocks (no server-side timeout) until the sync completes or is
  queued. Returns `{ enabled, queued, ran }` (with `enabled: false`
  if sync is disabled in config).
- `GET /sync/status` — returns `{ isSyncRunning, currentDuration,
  lastCompletedAt, lastCompletedAgo, lastCompletedDurationMs,
  lastCompletedPull, lastCompletedPush }`.

We only need `POST /sync/run` for the trigger path. The blocking
behaviour means we can use the response itself as the
"sync finished" signal — no polling required.

`syncData` we send: `{ type: "bestool", urgent: true }`. `type` is a
free-form tag stored in the sync session reason for log correlation
(matches the pattern of the scheduled task and the subcommand —
`scheduled` / `subcommand` / `userRequested`). `urgent: true` matches
what the desktop client sends and what the internal CLI subcommand
sends.

### Locating the sync service

Reuse `bestool_tamanu::services::expected` with `ApiServerKind::Facility`,
then pick the expectation named `tamanu-facility-sync` (systemd) or
`tamanu-sync` (pm2). Both already exist in `services.rs:262-273` for
the facility branch.

That gives us:
- The unit/process name for log tailing.
- Confirmation that the deployment is a facility (we already bail on
  central via `config.is_facility()` before this).

### Reading the sync port

Extend `bestool_tamanu::config::structure::Sync` with an optional
`sync_api_connection` block:

```rust
pub struct Sync {
    pub host: Option<Url>,
    pub sync_api_connection: Option<SyncApiConnection>,
}

pub struct SyncApiConnection {
    pub host: Option<String>,   // default "http://localhost"
    pub port: Option<u16>,      // default 4100
}
```

Defaults match `tamanu/packages/facility-server/config/default.json5`.

Helper on `TamanuConfig`: `sync_api_url()` -> `Url` that builds the
base URL from the config or falls back to `http://localhost:4100`.

### Log tailing

Spawn the existing `journalctl -fu tamanu-facility-sync.service` (or
pm2 log file tailing) and stream its stdout to ours line-by-line.

For systemd: use the same shape as `logs.rs::run_journalctl` —
`journalctl -u <unit> -n <lines> -f --output=cat`. The journal access
preflight (`journalctl_command()`) added on the
`fix-logs-caddy-alone` branch is the right thing to reuse, but it
isn't on main yet. For now we'll call `journalctl` directly; the
unprivileged-read case is a known existing issue that the other PR
covers, and once that lands the sync command will pick up the same
helper.

For pm2: lift the same `tail_files` pipeline that `logs.rs` uses
(read last N lines + follow), or — simpler — shell out to
`pm2 logs <name> --lines <n>` and pipe its output. `pm2 logs` already
formats per-stream and handles rotation, so that's the lighter path.
Decision: use `pm2 logs` for parity with the operator's mental model
("same thing I'd type manually").

The log child process is killed when:
- the trigger HTTP request returns (success or failure), or
- the user presses ctrl+c.

### Triggering the sync

`reqwest::Client` (already a dep), no timeout on the request itself.
Body `{ "syncData": { "type": "bestool", "urgent": true } }`. Spawn
this as a tokio task; in the foreground, pipe the log child's stdout
to our stdout until the task completes.

On completion:
- Print the result (`Sync completed`, `Sync queued`, `Sync disabled`,
  `Sync failed: <error>`) on a fresh line below the logs.
- Kill the log child.
- Exit with `0` for `ran || queued`, non-zero for `!enabled` or HTTP
  error.

`queued` is technically a non-completion — central is busy and asked
us to retry later. We could mirror the existing facility-server
subcommand and loop until `ran`, but that hides what's happening
and makes the command surprising. For v1 we report `queued` and exit
0; the operator can re-run.

### Ctrl+c

We rely on tokio's default ctrl+c handler for the foreground task.
Killing the child journalctl/pm2 process when the future is dropped
needs explicit handling: we spawn it via `tokio::process::Command`
with `kill_on_drop(true)`.

## Implementation steps

1. **Config extension** — add `SyncApiConnection` to
   `crates/tamanu/src/config/structure.rs` with the two optional
   fields and a `sync_api_url()` helper on `TamanuConfig`. Add unit
   tests for: defaults, host override, port override, missing block,
   non-facility (no `Sync` at all).

2. **Cargo features** — add `tamanu-sync` feature in
   `crates/bestool/Cargo.toml`, included in the `tamanu` umbrella.
   Deps: `__tamanu`, `tamanu-config`, `dep:bestool-postgres` (only if
   we end up reusing the config-load path that needs it — likely
   not, so omit). reqwest is already a non-optional dep.

3. **Subcommand wiring** — add `Sync(SyncArgs)` to the `Action` enum
   in `crates/bestool/src/actions/tamanu.rs` behind
   `#[cfg(feature = "tamanu-sync")]`.

4. **`sync.rs` implementation** — new file
   `crates/bestool/src/actions/tamanu/sync.rs`:
   - Load config, bail if not facility.
   - Determine supervisor.
   - Build trigger URL from config.
   - Spawn log child (`journalctl` or `pm2 logs`) with
     `kill_on_drop(true)`, pipe stdout through to ours via a small
     copy task.
   - Fire `POST /sync/run` with no timeout. Await result.
   - Print final status line; drop log child; return appropriate
     exit code (via `bail!` for non-OK).

5. **Tests**:
   - Config parsing: see step 1.
   - End-to-end: skip — needs a running facility. Document manual
     test in the commit body.

## Open questions

- Should `--timeout` poll `/sync/status` to differentiate "the request
  is hung" from "the sync is genuinely still running"? For v1, no —
  the timeout simply cancels both tasks and exits. If this becomes a
  pain we can add a heartbeat probe in a follow-up.
- Do we want a `--type` flag so the operator can label the sync
  reason in central's logs? Probably not worth the surface area —
  `bestool` is enough.

## What ships

- New file: `crates/bestool/src/actions/tamanu/sync.rs`.
- Edits: `crates/tamanu/src/config/structure.rs` (config additions
  and tests), `crates/bestool/src/actions/tamanu.rs` (subcommand
  registration), `crates/bestool/Cargo.toml` (feature flag).
- No new dependencies.
