# Fold tamanu doctor into the alertd daemon

Goal: bring `tamanu doctor --send`'s periodic healthcheck push into the
`bestool-alertd` daemon so we only run one long-lived process. Keep the
interactive `tamanu doctor` CLI command — drop only `--send`.

## Crates

Split shared code out of `bestool` and `bestool-alertd` into two new crates:

- `bestool-canopy` — `CanopyClient`, `NewEvent`, `Severity`,
  `DEFAULT_CANOPY_URL`, `TAILSCALE_URL`, `CERT_RENEW_AFTER`, mTLS + tailscale
  probe. Lifted unchanged from `crates/alertd/src/canopy.rs`.

- `bestool-tamanu` — `TamanuConfig` + loader, `find_tamanu`,
  `ConnectionUrlBuilder`, `server_info::{fetch_device_key, get_or_create_server_id}`,
  and the doctor check infrastructure (`Check`, `CheckStatus`, `OverallResult`,
  `CheckContext`, `CheckEntry`, all 17 check modules, `ServerFacts::gather`,
  `build_payload`). Exposes a `run_checks(ctx) -> (Vec<(Check, bool)>, OverallResult)`
  plus a `build_status_payload(facts, results, overall) -> Value`.

- `bestool-alertd` — same crate, now depends on `bestool-canopy` (canopy module
  removed). Gains a `BackgroundTask` trait and a `with_task` builder on
  `DaemonConfig`. Daemon spawns one tokio task per registered plugin.

- `bestool` — depends on all three. Wires `impl BackgroundTask for DoctorTask`
  using check infrastructure from `bestool-tamanu`. Registers it on the
  `DaemonConfig` when `Command::Run` is invoked.

## Plugin interface (in bestool-alertd)

```rust
pub struct TaskContext {
    pub pg_pool: bestool_postgres::Pool,
    pub http_client: reqwest::Client,
    pub canopy_client: Option<Arc<CanopyClient>>,
}

pub trait BackgroundTask: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn interval(&self) -> Duration;
    fn run<'a>(&'a self, ctx: &'a TaskContext)
        -> futures::future::BoxFuture<'a, miette::Result<()>>;
}

impl DaemonConfig {
    pub fn with_task(self, task: Arc<dyn BackgroundTask>) -> Self;
}
```

Daemon behaviour per task:
- spawn a tokio task that ticks every `interval()`,
- `metrics::record_activity()` around each tick so watchdog counts it,
- log errors with `LogError` but never propagate (one failed check should not
  kill the daemon),
- skip ticks while DB pool is unavailable? No — let the task itself decide.

## Doctor plugin (in bestool)

- Interval: 1 minute (matches existing cron cadence).
- On each tick:
  1. Build a `CheckContext` from cached `TamanuConfig` + `tamanu_root` (captured
     at daemon startup; checks use them read-only) and a fresh DB connection
     from the pool.
  2. Run all checks via `bestool_tamanu::doctor::run_checks`.
  3. Resolve `metaServerId`. Skip the post if absent.
  4. Gather `ServerFacts` and build the canopy payload.
  5. POST via the shared `CanopyClient` (`ctx.canopy_client`). If `None`,
     log-and-skip.
- Use the shared `ctx.http_client` for HTTP checks (`tamanu_http`,
  `http_errors`, `time_sync` if it does HTTP). This is the connection-pooling
  win — same `reqwest::Client` across ticks so TCP/TLS stays warm.

## CLI changes

- `tamanu doctor`: drop `--send` and `--canopy-url`. Keep `--json` and
  `--check`. Still single-shot, still renders, still uses fresh DB connection +
  one-off `reqwest::Client`.
- `tamanu alertd run`: no flag changes. Doctor task is auto-registered. If
  there's a reason to disable it (e.g. tests), add a `--no-doctor` flag.

## Execution order

1. Create `bestool-canopy`, move canopy module out of alertd. Update alertd +
   bestool imports. Verify build + tests on Linux and Windows targets.
2. Create `bestool-tamanu`. Move `config`, `find`, `connection_url`,
   `server_info` (the tamanu-level one), then the doctor checks tree
   (`doctor/check.rs`, `doctor/checks.rs`, `doctor/checks/*.rs`,
   `doctor/server_info.rs`). Update bestool imports throughout. Tests still
   need a database via `DATABASE_URL=postgresql://localhost/tamanu_meta`.
3. Add `BackgroundTask` trait + `with_task` to `bestool-alertd`. Daemon spawns
   the tasks; `TaskContext` carries `pg_pool`, `http_client`, `canopy_client`.
4. Implement `DoctorTask` in bestool. Wire it into `tamanu alertd run`.
5. Drop `--send` and `--canopy-url` from `tamanu doctor`. Strip the canopy
   posting code path from `doctor.rs` (now lives in the plugin).
6. Update any cmd snapshot tests (`crates/bestool/tests/cmd/*`).

## Open questions / follow-ups

- Should the doctor task be opt-out (`--no-doctor`)? Default yes, with a flag.
  Not needed yet — no caller currently has reason to disable.
- Should `DoctorTask` cache the `ServerFacts` that don't change tick-to-tick
  (timezone, pg version)? Probably yes — re-querying `SELECT version()` every
  minute is wasteful. Out of scope for v1.
- The current `--check NAME` filter on `tamanu doctor` doesn't apply in the
  daemon (the daemon always runs the full registry). Intentional.
