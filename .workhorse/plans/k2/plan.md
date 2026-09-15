# Move the alertd daemon into the bestool binary

Relocation only: `bestool-alertd` keeps the checks and the machinery they are built from, the daemon that schedules them moves into `bestool` behind the existing `alertd` feature.
No behaviour changes.

## The seam

Verified against the tree before starting:

- `doctor/` outside `task.rs` has **zero** references to the crate root or to any moving module. Every `super::` path inside `doctor/` resolves within `doctor/`.
- `doctor/task.rs` reaches out for `crate::tasks::TaskEndpointHandler` and `crate::{BackgroundTask, TaskContext, TaskEndpoint, TaskEndpointResponse}` — all daemon-side, and it moves with them.
- The daemon reaches into `doctor/` only for `MetricsSnapshot`, `Stat`, `StatKind`, `StatusCounts` (rendering `/metrics`) and `DoctorMetricsHandle` (defined in `task.rs`, so it moves too).
- Every `bestool_alertd::{commands, http_server, DaemonConfig, BackgroundTask, ...}` use site in `bestool` is already `#[cfg(feature = "alertd")]`. `tamanu doctor` uses only staying items.

Test split: ~370 tests stay under `doctor/`, ~22 move (11 `http_server`, 10 `doctor/task.rs`, 1 `windows_service`).

## Decisions

**Layout.** The moved daemon lands in a new top-level `crates/bestool/src/alertd/`, gated by `#[cfg(feature = "alertd")]` in `lib.rs`. `actions/alertd.rs` keeps only the clap CLI and calls into it, so the daemon library stays separate from the CLI action layer.

**Flatten.** `doctor::` is hoisted to the alertd crate root in the same major bump: consumers write `bestool_alertd::checks`, `::check`, `::sweep`, `::subject`, `::stat`, `::heal`, `::progress`, `::server_info`. The `super::` paths inside the hoisted modules survive untouched — `super` of `crate::checks` is `crate`, exactly as `super` of `crate::doctor::checks` was `crate::doctor`.

**Version/UA.** The daemon builds its own HTTP clients, but the identity string stays in `bestool-alertd` as `USER_AGENT`, so the outbound User-Agent remains `bestool-alertd/<alertd version>` rather than silently becoming bestool's. A test in the checks crate holds that. `DaemonConfig`'s `binary_version` is just bestool's own `CARGO_PKG_VERSION`, which is the only thing it was ever set to.

**Versions.** Left to release-plz; the breaking change is marked in the commit message.

## Steps

- [x] Hoist `doctor/*` to the alertd crate root; fold `doctor.rs`'s re-exports into `lib.rs`
- [x] Move `daemon`, `http_server` (+ subtree), `tasks`, `backup`, `child_confinement`, `windows_service`, `context`, `metrics`, `commands` (+ subtree) to `crates/bestool/src/alertd/`
- [x] Move `doctor/task.rs` to `crates/bestool/src/alertd/doctor.rs`
- [x] Move `DaemonConfig` and `LogError` out of alertd's `lib.rs` into the bestool side
- [x] Rewrite import paths on both sides
- [x] Split `Cargo.toml`: drop `axum`, `tokio-stream`, `tower-http`, `sd-notify`, `win32job`, `windows-service` (and the already-unused `bestool-kopia`) from alertd; add what the daemon needs to bestool's `alertd` feature
- [x] Update the four `tamanu doctor` files and `self_update/task.rs` for the new paths
- [x] `cargo check`/`clippy`/`test` on Linux; `cargo check` for a Windows GNU target
- [x] Confirm `tamanu doctor` still builds with `alertd` off

## Surfaced by the move

Moving a library into a binary turns `pub` items into dead-code candidates, which
surfaced things the library boundary had been hiding:

- `RestartTrigger` and `TaskContext::restart` are live only on Windows, where the
  self-update task replaces the binary. Gated `#[cfg(windows)]` so the code
  exists only where it is used, rather than annotated as dead.
- `windows_service::install_service` had no callers and defaulted to service args
  (`service`) that would not have worked for bestool, which passes
  `alertd service`. Deleted.

## Raised in review

- The `alertd` feature no longer compiled on its own: the daemon code moved into
  `bestool`, where `bestool-postgres`, `bestool-tamanu` and `node-semver` are
  optional deps that the feature did not enable. It built on `main` because the
  code lived in `bestool-alertd`, which depends on them unconditionally. Fixed by
  adding them to the feature, plus a CI job that builds this configuration, which
  nothing else in CI covered.
- `DaemonConfig::database_url` is now `Redacted<String>`. Its own doc said
  "retained for redacted display" while the hand-written `Debug` printed the
  postgres URL, password and all. Nothing reads the field to connect.

## Mandating the pool

The daemon opened a connection pool at startup and threaded it through
`DaemonConfig`, `InternalContext` and `TaskContext` — and then nothing read it.
The sweep opened its own connection with `connect_one` every tick instead, so a
daemon sweeping every minute paid for a fresh connect each time while the pool
sat unused. The dead field was the visible end of an unfinished wire, so this
card connects it rather than deleting it.

A sweep takes its connection from a pool, and only from a pool: the
`connect_one` fallback is gone, so there is one way in rather than two. Both
callers supply one — the daemon reuses a pool across sweeps, and the `doctor`
CLI builds one for its run, which costs it nothing because `connect_one` was
itself a `create_pool` that took one connection and threw the pool away.
`CheckContext::db` is a pooled connection; the `SweepDb` enum that spanned the
two ways in is gone with the second way.

The pool belongs to the doctor task, not to `DaemonConfig`. Building one
requires the database to be up, so a daemon started while postgres is down can't
be handed one — it has to be able to build one later. The task builds its pool
on the first sweep that reaches the database, keyed by the URL so an in-place
upgrade rebuilds it, and retries every tick until then. That also means startup
no longer touches the database at all.

What this does not change:

- `db_connect` opens its own connection with `tokio_postgres::connect` to
  measure connect latency, and never goes through the pool, so a pool that
  cannot hand out a connection can't mask a database outage.
- A failed acquire warns and leaves `db` as `None`, so DB-dependent checks skip.
- Postgres is still not required for the daemon to start — more so than before,
  since startup no longer attempts a connection.
- mobc caps an acquire at 30 seconds by default, so an unreachable database
  fails the acquire rather than stalling the sweep.

Each check takes its own connection, rather than sharing one. Checks already run
concurrently, so sharing meant their queries pipelined onto a single backend and
a sweep cost the sum of its DB work instead of the longest piece of it. A pool
that only ever yields one connection is no pool at all, so `CheckContext` holds
the pool and `ctx.db()` acquires per check, giving it back when the check ends.
The pool bounds how many run at once; a check that has to wait simply starts
later. A test holds this: two connections taken at once must report different
backend PIDs.

One test had to change with it. `grades_a_seeded_gap_against_central` seeded its
fixture inside an uncommitted transaction and relied on the check sharing that
connection to see it. It now commits the seed and deletes it afterwards, which
is what a check reading through its own connection requires. It also deletes the
probe rows before seeding, so a run that dies before its cleanup doesn't poison
the next one.

## Second review round

- The daemon's HTTP client factory was still in the checks crate, which the
  daemon reached back across the seam for. The clients are built in
  `crate::alertd` now; only `USER_AGENT` stays behind, because it has to carry
  the checks crate's version rather than the binary's. A test in that crate
  holds the two together.
- `DaemonConfig::with_binary_version` was a setter for a value identical to the
  default, now that both resolve `CARGO_PKG_VERSION` in the same crate. It only
  existed while the config lived across a crate boundary. Removed, with its two
  call sites.
- Two comments were raised against a snapshot from before the pool was wired up,
  and no longer hold: `TaskContext::pg_pool` is read by the doctor task, not
  dead, and carries no annotation. The suggestion to swap `restart`'s
  `cfg(windows)` for `expect(dead_code)` rested on matching the sibling field's
  strategy, which no longer exists.

## Third review round

All four were consequences of giving each check its own connection.

- The seeded-gap test was writing for real to whichever database answered at
  the central URL. CI has no `tamanu-central`, so the only machines it ran on
  were developer and ops boxes with a live Tamanu, and its settings delete was
  not probe-scoped. It restores the prior value verbatim now and is gated behind
  `BESTOOL_TEST_DESTRUCTIVE_DB`. Verified by seeding a value, running it, and
  confirming the value survived.
- The pool kept mobc's default of ten connections while twenty-two checks asked
  for one each. The overflow queued, and an acquire that timed out returned
  `None`, which five checks report as a failed check — contention would have
  raised a database-down alert. The pool is sized to the fan-out, so a failed
  acquire means what it did before: the database is unusable.
- The sweep's setup connection was held from before the checks until the facts
  query after them, occupying a slot for the contended phase. It is handed back
  once the setup queries are done.
- `pool_for` held its mutex across `create_pool`, which talks to the database;
  a concurrent `recompute` stalled behind it for the connect timeout on every
  tick while postgres was down.

## Fourth review round

The pool has now been through three rounds, and the reason is worth recording:
round three flagged that checks were queueing behind a ten-connection pool, and
the fix taken then was to size the pool up to the fan-out. That removed the
queueing by bursting twenty-odd backends a minute at the deployment's own
database — trading a latency problem for a capacity one, which round four
rightly called the healthcheck being able to cause the outage it reports. The
mistake was optimising the fan-out without a budget for connections against a
production database.

The shape it settles into:

- Eight connections, and checks queue for one. Several run at a time, which is
  the point, without the sweep ever being a meaningful share of a cluster's
  connection limit.
- Queueing is only safe because a database that is actually down leaves the
  checks with no pool at all. The sweep takes a connection itself first, and
  only passes the pool on if that worked, so checks never queue for something
  that cannot arrive.
- Idle connections outlive the gap between sweeps and age out when the daemon
  goes quiet, so reuse survives tick to tick without holding backends open.

Also fixed: `pool_for`'s failure branch was clearing the cache entry a
concurrent sweep may have just written; the seeded-gap test could adopt its own
leftover setting as the deployment's prior value and make it permanent, and now
declines rather than editing a setting it didn't create; and `DaemonConfig`
carried a `database_url` nothing read and a `binary_version` that is a constant.

Left alone: `perform_sweep`'s nine positional parameters want a builder, which
is a signature change better made alongside the check-signature work than
bolted on here. The `alertd` feature enabling `bestool-tamanu` is not
avoidable — the daemon itself uses pm2 job breakaway, the seedling endpoint and
the tag cache — so `alertd-tamanu` narrowing to `tamanu-config` is a real
consequence of where that code now lives, not an oversight.
