# alertd check-declared metrics + munin/prometheus endpoint

## Goal

Surface the numeric telemetry the doctor healthchecks already gather (cert
validity, FHIR queue depth, sync staleness, active sessions, snapshot-table
counts, disk/memory/load, …) as first-class metrics that munin can harvest,
served off alertd's existing `/metrics` route. Checks *declare* typed stats;
one renderer emits either munin-native text or prometheus text depending on the
request's `Accept` header. A global status census (passing/warning/failing/
skipped/broken) is derived automatically from each sweep.

Munin is the priority consumer (there is no munin-prometheus bridge plugin in
the fleet), so a thin munin plugin ships in the bestool deb.

Separately, report a top-level `munin: bool` fact to canopy when munin-node is
present on the host.

## Background (current state)

- Checks live under `crates/alertd/src/doctor/checks/`; each returns one
  `Check` (`doctor/check.rs`) carrying `status`, `summary`, and a loosely-typed
  `details: Map<String, Value>` (canopy-bound JSON). Numeric data today is only
  in `details` — as scalars, dimensioned maps (`fhir_jobs.by_status`,
  `http_errors.by_code`), or arrays of objects (per-cert, per-mount).
- The doctor task (`doctor/task.rs`) runs a sweep every 60s and caches the
  latest `SweepResult`; it applies canopy's severity ceilings on read
  (`capped()`), and already exposes `/tasks/doctor/{latest,recompute}`.
- The HTTP server (`http_server.rs`, axum) has a `/metrics` route
  (`endpoints/metrics.rs`) that returns prometheus text from a global registry
  holding a single gauge, `bes_alertd_last_activity_unix` (`metrics.rs`) — a
  liveness signal for `/health` and the watchdog.
- `ServerState` (`http_server/state.rs`) is threaded from `DaemonConfig`
  (`daemon.rs`); `backups: Option<Arc<BackupRegistry>>` is the precedent for an
  optional shared handle reaching a handler.
- Pure-fact contributors already exist: the `ips` check is `on_wire: false`,
  always passes, and attaches top-level payload facts via `payload_extras`,
  which `build_payload` (`sweep.rs`) lifts alongside `osTimezone`.
- `systemd::is_enabled(unit)` exists (`crates/tamanu/src/systemd.rs`) with a
  non-Linux stub.
- The deb is assembled by hand in `.github/workflows/release-bestool.yml`
  (installs the systemd unit, shell completions, docs via `install -D`; runs a
  `postinst` that does `systemctl daemon-reload`).

## Design

### Stat type

New `crates/alertd/src/doctor/stat.rs`:

```rust
pub enum StatKind { Gauge, Counter }

pub struct Stat {
    name: &'static str,                  // snake_case; valid prom name & munin field
    value: f64,                          // ints and floats both fit
    kind: StatKind,
    labels: Vec<(&'static str, String)>, // static keys, dynamic values, insertion order
    help: Option<String>,                // prom HELP / munin field label
}
```

Builder mirrors the existing `.with_detail` ergonomics:

```rust
Stat::gauge("age_seconds", age as f64).help("Sync lookup staleness")
Stat::counter("requests_total", n as f64)
Stat::gauge("jobs", n as f64).label("status", status)   // called once per status
```

`Check` gains `stats: Vec<Stat>` plus `with_stat(Stat)` / `with_stats(iter)`;
all `Check::{pass,skip,warning,fail,broken}` constructors initialise it empty.
`details` (canopy JSON) and `stats` (metrics) stay separate — a check computes
a number once and may attach it to both. Units live in the stat *name* by
prometheus convention (`_seconds`, `_bytes`); no separate unit field.

### Naming

- Prometheus: `bes_alertd_<check>_<stat>{label="…"} <value>`, e.g.
  `bes_alertd_fhir_jobs_jobs{status="Queued"} 4`. `# HELP`/`# TYPE` emitted per
  metric.
- Munin: one multigraph per check, id `bes_alertd_<check>`, `graph_category
  bestool`; each stat is a field. A labelled stat expands to one field per
  label-value — field id `<stat>_<value…>` sanitised to `[a-z0-9_]`, field
  label = the human value. Field type: `Gauge`→`GAUGE`, `Counter`→`COUNTER`.

### Global status census

Derived from the cached sweep, capped to canopy's severity ceilings so it
matches what operators see elsewhere:

```
bes_alertd_checks{state="passing|warning|failing|skipped|broken"} N   # prom
bes_alertd_checks  → munin graph, same five fields + total                # munin
```

`active` = ran (total − skipped); the full breakdown lets any subset be
derived.

### Endpoint & format negotiation

Reuse the existing `/metrics` route (no new path):

- Default / `*/*` / prometheus Accept → prometheus text
  (`text/plain; version=0.0.4`). Existing scrapers keep working; they just get
  the sweep-derived series in addition to the liveness gauge.
- `Accept: text/x-munin` → munin native text.
- Munin's two-call protocol: `?config` → munin config lines only; bare request
  → values. (No dirtyconfig — keep config and fetch as separate, unambiguous
  responses. The plugin makes both calls.)
- The `bes_alertd_last_activity_unix` liveness gauge is always included: as-is
  in prom mode; as a `bes_alertd_daemon` munin graph in munin mode.

Config and values are rendered from the *same* snapshot per scrape, so they are
always internally consistent. Known munin wrinkle to note in code: a check
flipping skipped↔ran changes its field set (rare on a stable host); v1 accepts
this rather than persisting a superset.

### Wiring

- `MetricsSnapshot { computed_at, stats: Vec<(&'static str /*check*/, Stat)>,
  counts: StatusCounts }` and `StatusCounts` defined alongside the doctor task.
- `DoctorMetricsHandle` (an `Arc` over the doctor task's inner) with
  `async fn snapshot(&self) -> Option<MetricsSnapshot>`: reads the cached sweep,
  applies `capped()` for the status counts, and collects each check's `stats`.
  `DoctorTask::metrics_handle(&self)` hands one out.
- `ServerState` gains `metrics: Option<DoctorMetricsHandle>`, threaded through
  `DaemonConfig` and `start_server` exactly like `backups`. The bestool binary
  (`crates/bestool/src/actions/alertd.rs`) grabs the handle from the concrete
  `DoctorTask` before boxing it into the task list and passes it into the
  daemon config.
- Rendering lives in the http layer: `http_server/metrics_render.rs` with
  `render_prometheus(&MetricsSnapshot, liveness) -> String` and
  `render_munin(&MetricsSnapshot, liveness, config: bool) -> String`, plus munin
  field/graph-name sanitisation. `endpoints/metrics.rs` negotiates format from
  the `Accept` header and `?config`, pulls the snapshot from `ServerState`, and
  renders. When no sweep is cached yet, emit just the liveness gauge (and, in
  munin config mode, its graph).

### `munin` payload fact

New host-level fact-check `doctor/checks/munin.rs`, mirroring `ips`:
`on_wire: false`, always `Pass`, attaches top-level `munin: bool` via
`with_payload_extra("munin", detected)`. Detected = `munin-node.service`
enabled (`systemd::is_enabled("munin-node")`) OR the `munin-node` binary is on
`PATH`; non-Linux → `false`. The canopy status payload is free-form JSON
(`status()` posts a `serde_json::Value`), so no request-schema change is needed;
canopy surfacing the field is its own concern, out of scope here.

### Munin plugin in the deb

- Source in-repo at `contrib/munin/bestool_alertd`: a thin POSIX-sh curl
  wrapper. `config` arg → `GET /metrics?config`; otherwise `GET /metrics`; both
  send `Accept: text/x-munin`. Target base tries `http://[::1]:8271` then
  `http://127.0.0.1:8271` (mirroring the daemon's v6-first bind order), with an
  `env.url` override (munin `plugin-conf.d` convention).
- `release-bestool.yml` installs it to `/usr/share/munin/plugins/bestool_alertd`
  (mode 0755) — munin's available-plugins dir.
- The existing `postinst` gains: if munin-node is installed
  (`/etc/munin/plugins` exists), symlink the plugin active and reload
  munin-node; otherwise a no-op. Mirrors shipping the alertd unit disabled —
  present, wired up only where wanted.

## Implementation steps

1. `doctor/stat.rs`: `Stat`, `StatKind`, builders (`gauge`, `counter`, `label`,
   `help`); register `pub mod stat;` in the doctor module root. Unit tests for
   the builder and label ordering.
2. `doctor/check.rs`: add `stats` field + `with_stat`/`with_stats`; init in all
   constructors.
3. `MetricsSnapshot` + `StatusCounts` + `DoctorMetricsHandle::snapshot()` in the
   doctor task; `DoctorTask::metrics_handle()`. Test status-count derivation
   respects capping.
4. `http_server/metrics_render.rs`: prometheus + munin renderers and name
   sanitisation. Golden tests: prom output, munin config, munin values, label→
   field expansion, name sanitisation.
5. `http_server/state.rs` + `daemon.rs` + `actions/alertd.rs`: thread
   `metrics: Option<DoctorMetricsHandle>` through, mirroring `backups`.
6. `endpoints/metrics.rs`: rewrite `handle_metrics` to negotiate `Accept` +
   `?config`, render from the snapshot, always include the liveness gauge.
   Tests: prom default, munin values, munin config, no-sweep-yet.
7. Instrument the named checks with `.with_stat(...)`: `caddy_certs`,
   `fhir_jobs`, `sync_lookup`, `sync_sessions`, `sync_snapshot_tables`,
   `disk_free`, `memory`, `load`, `db_connect`, `http_errors`. Extend existing
   per-check tests to assert the emitted stats.
8. `doctor/checks/munin.rs`: the `munin` fact-check; register in `checks::all()`
   as a host-level, `on_wire: false` check. Test detection wiring.
9. `contrib/munin/bestool_alertd` plugin script; edit `release-bestool.yml` to
   install it; extend `postinst` with the conditional munin-node symlink+reload.

## Scope

- **This branch:** everything in the steps above — mechanism, endpoint, global
  census, munin fact, plugin in the deb, and the named-set instrumentation.
- **Stacked follow-up:** instrument the remaining numeric checks (the `*_errors`
  family counts, `inodes`, `btrfs`, `external_users`, `tamanu_http`, `pg_tuning`,
  `sync_session_errors`, …) and add p50/p99 snapshot-table *size* percentiles
  (net-new SQL — no size measurement exists today).
