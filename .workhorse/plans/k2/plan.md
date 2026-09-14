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

Still to decide: the sweep shares one connection between all its DB checks, so
their queries pipeline onto a single backend. Now that a pool is always present,
each check could take its own and run properly in parallel — but that changes
every DB check's signature and raises a sweep's peak backends from one to the
pool's limit, so it belongs with the check-signature work rather than here.

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
