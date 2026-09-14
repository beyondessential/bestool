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

**Version/UA.** `VERSION`, `http_builder`, `http_client` stay in `bestool-alertd`, so the outbound User-Agent remains `bestool-alertd/<alertd version>` rather than silently becoming bestool's version. `DaemonConfig`'s `binary_version` fallback becomes bestool's own `CARGO_PKG_VERSION`, which is what every call site already overrides it to.

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

## Surfaced by the move, not resolved here

Moving a library into a binary turns `pub` items into dead-code candidates, which
surfaced three things the library boundary had been hiding:

- `TaskContext::pg_pool` is written but never read, on either platform. Every
  check that needs the database opens its own connection from the sweep's URL.
  Left in place behind an `expect(dead_code)` with a reason, since it is offered
  plumbing rather than a mistake; worth deleting if no task claims it.
- `RestartTrigger` and `TaskContext::restart` are live only on Windows, where the
  self-update task replaces the binary. Marked `cfg_attr(not(windows), expect(…))`.
- `windows_service::install_service` had no callers and defaulted to service args
  (`service`) that would not have worked for bestool, which passes
  `alertd service`. Deleted.
