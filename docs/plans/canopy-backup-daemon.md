# canopy backup via the alertd daemon

Implements the delegation behaviour added to [BAK](../../.workhorse/specs/canopy/backup.md):
`bestool canopy backup` prefers the running daemon, streaming the run's progress
and outcome back, and falls back to running locally when no daemon is reachable
or `--no-daemon` is given. Mirrors how `bestool tamanu doctor` integrates with
the daemon. Restore stays local. Shape: stream live status, attach to an
in-progress run rather than failing or queueing, and surface running runs in the
daemon's status.

## Context

- The daemon (`bestool-alertd`) serves loopback HTTP on `:8271`
  (`crates/alertd/src/http_server.rs`): a generic task dispatch
  `GET /tasks/{task}/{endpoint}` returning `TaskEndpointResponse::{Json,
  JsonLines(BoxStream<Value>), Error}`, plus `/status`.
- `tamanu doctor` already does daemon-or-local with `--no-daemon` and an NDJSON
  stream, falling back to local on any daemon error
  (`crates/bestool/src/actions/tamanu/doctor.rs`).
- The daemon already runs backups in-process: `DoctorTask` calls an injected
  `BackupDispatch` with Canopy's `backup_now` list;
  `crates/bestool/src/actions/alertd.rs` wires it to
  `canopy::backup::run_backup(&type, None, None)` under an overlap-guard HashSet.
- `run_backup` (`crates/bestool/src/actions/canopy/backup.rs`) holds a
  cross-process per-type file lock for the whole run.

## Backup registry (daemon, the core new piece)

A registry in the alertd crate, keyed by backup type, is the single way a backup
runs inside the daemon — used by both the Canopy-triggered path and the CLI
`run` endpoint. Each in-progress run holds:

- run id, type, started-at, and the latest status event;
- a `tokio::sync::broadcast` sender so any number of subscribers receive live
  status.

`ensure_run(type) -> Subscription` is start-or-attach: if the type isn't
running, it starts a run (via an injected runner, below) and registers the
handle; if it is, it returns a subscription to the existing run. A subscription
first yields the latest known status (so a late attacher isn't blank), then live
events, then the terminal event. The entry is removed when the run ends (keeping
the terminal outcome briefly so in-flight subscribers see it). This replaces the
overlap-guard HashSet: attaching, not failing or queueing, is the behaviour for a
second request.

The runner is injected from bestool (where `run_backup` lives):

```rust
// alertd-side
type BackupRunner =
    Arc<dyn Fn(String, mpsc::Sender<serde_json::Value>) -> BoxFuture<'static, ()> + Send + Sync>;
```

bestool's runner calls `run_backup(type, None, None, Some(sink))`, adapting
`BackupEvent`s to JSON. The registry owns the fan-out and lifecycle; the runner
just produces one run's event stream.

## run_backup progress sink (phase status + heartbeat)

`run_backup` gains `progress: Option<mpsc::Sender<BackupEvent>>` (`None`
preserves today's behaviour). Events:

```rust
pub enum BackupEvent {
    Started { run_id: String },
    Phase(&'static str),   // connect, pre-hooks, prepare, snapshot, report
    Done { snapshot_id: Option<String>, uploaded_bytes: Option<i64> },
    Failed { error: String },
}
```

`run_backup` already has the run id, the phase boundaries, the snapshot result
(bytes/id), and the lock-skip path, so it emits these directly with no change to
how kopia is invoked. The registry adds a periodic heartbeat into the broadcast
so the connection stays alive during the long snapshot phase.

Byte-level live progress is **not** in v1: verified that kopia emits no progress
output when its stderr is not a TTY (the daemon's case) — only the opening
"Snapshotting…" line. Surfacing live counters would mean running kopia under an
allocated PTY and parsing its human progress line, which is fragile and platform-
specific; deferred as a follow-up. Phase status + the final byte count is the
status v1 streams.

## `run` endpoint

`GET /tasks/backup/run?type=X` → `ensure_run(type)` then stream the subscription
as NDJSON, mirroring doctor's events:

```json
{"event":"started","runId":"…","attached":false}
{"event":"phase","phase":"snapshot"}
{"event":"heartbeat"}
{"event":"done","success":true,"snapshotId":"…","uploadedBytes":123}
{"event":"error","message":"…"}
```

`attached:true` on the first event tells the client it joined an already-running
backup. The endpoint needs the `type` query parameter, which the dispatch
doesn't currently pass to handlers — extend `TaskContext` with the request's
query parameters (`BTreeMap<String,String>`), populated in
`handle_task_endpoint`. Missing/unknown `type` → `Error{400}`.

## Running backups in `/status`

`/status` (`crates/alertd/src/http_server/endpoints/`) gains a `backups.running`
list from the registry: type, run id, started-at, and latest phase/progress. So
an operator (or a `bestool` status view) can see what's backing up right now.

## CLI delegation

`BackupArgs` gains `--no-daemon`. `run()`:

- `--no-daemon` → `run_backup(type, …, None)` locally (today's path; no attach —
  attach is a daemon-registry feature and can't cross processes);
- otherwise `GET …/tasks/backup/run?type=X` (`127.0.0.1` and `[::1]`) with a
  short connect timeout; on connect, stream NDJSON and render status/progress,
  noting when it attached to an existing run, exiting by the terminal event;
- on any connect/transport error, fall back to the local path.

Reuse doctor's client helpers (`DAEMON_BASE`, the NDJSON line drain, the
fall-back-on-error structure).

## Unifying the Canopy-triggered path

The existing `BackupDispatch` wiring routes through the registry's `ensure_run`
too, so a Canopy-scheduled run and a CLI-initiated one share one run and one
broadcast (the CLI attaches to a scheduled run already in flight, and vice
versa), and both appear in `/status`.

## Security / lifecycle

Loopback-only, no auth — same as doctor and the control endpoints. The
cross-process file lock still guards the daemon-vs-`--no-daemon` case; within the
daemon the registry guarantees one run per type with attach.

## Out of scope

`bestool canopy restore` — operator-interactive (clobber confirmation), local.

## Commit sequence

1. `run_backup` progress sink: phase + terminal events, `None` preserves
   behaviour; unit-test the event sequence.
2. `TaskContext` query parameters.
3. Registry (start-or-attach, broadcast, heartbeat, `running()`), `BackupRunner`
   type, and the `backup` task with the `run` endpoint, in the alertd crate.
4. `/status` running-backups list.
5. bestool wiring: inject the `run_backup`-backed runner; route the Canopy
   dispatch through the registry; register the task.
6. CLI `--no-daemon` + daemon delegation with local fallback; `./update-usage.sh`.

## Decisions (settled)

- **Stream phase status + heartbeat**, not fire-and-report — a backup runs far
  longer than a doctor sweep. Byte-level live progress is deferred (kopia emits
  none without a TTY).
- **Attach to an in-progress run** of the same type rather than failing or
  queueing.
- **Backup only**; restore stays local.
