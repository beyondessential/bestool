# Report checks and facts split by machine and application

Split every check and reported fact so each is filed against its true subject — the
machine, or an application on it — and push to Canopy in the split `StatusPayload`
format. Split out of `K1` and sequenced before it; nothing here should be redone by
the substrate work.

`SUBJ` (`.workhorse/specs/tamanu/subjects.md`) is this card's spec and the source of
truth for the catalogue split. It was authored on `K1` and carried onto this branch;
`SUB` (`substrate.md`) stays K1's and is deliberately absent here, so the SUB
cross-references were trimmed from `SUBJ` and `CHK` and return when K1 lands.

## Code anchors (current state)

- `crates/alertd/src/doctor/sweep.rs`
  - `perform_sweep` assembles a flat payload: calls `get_or_create_server_id()`
    (~line 403), gathers server facts, builds `ServerInfo`, then `build_payload`
    (~line 515) merges facts + lifted `payload_extras` + a single `health[]` array
    into one flat `Value`.
  - `SweepResult.payload: Value` is the flat object. `apply_severities` and
    `overall_from_payload` both read/write `payload["health"]`.
- `crates/alertd/src/doctor/checks.rs`
  - `entry!` macro categorises each check `@tamanu` / `db` / `host` (± `off_wire`).
    These encode **what inputs a check needs** (they drive skips on db-only,
    generic-db, and no-tamanu contexts), not whose subject it reports for.
  - `CheckEntry { name, on_wire, run, heal }` — no subject/scope field today.
  - `all()` registers 45 checks in CLI-render order.
- `crates/alertd/src/doctor/server_info.rs`
  - `ServerInfo` is the flat fact struct (26 fields); `gather()` builds it; several
    fields are non-`Option`. `bestool_version` lives here.
- `crates/tamanu/src/server_info.rs`
  - `get_or_create_server_id()` (line 172) resolves the id from the canopy
    registration first, then `/etc/tamanu/server-id`, else mints one.
- Call sites of `get_or_create_server_id()`: `sweep.rs` (~403) and
  `crates/bestool/src/actions/tamanu/doctor.rs:226`.
- `crates/alertd/src/doctor/task.rs` (~306) deserialises the flat `Value` into the
  typed `StatusPayload` via `serde_json::from_value` (leaning on `#[serde(flatten)]
  extra`), then pushes with `canopy.status(&server_id, &payload)` and caches the
  returned `check_severities`.
- CLI `doctor.rs` reads `payload["health"]` (~425) and `payload["serverId"]` (~401).

## The wire schema is already typed (no schema work needed here)

`bestool_canopy::schema` re-exports `bes_canopy_api::schema` (published crate
`bes-canopy-api` 1.0.0, generated from Canopy's OpenAPI). The split format is fully
present:

- `StatusPayload { applications: Option<HashMap<String, ApplicationReport>>,
  health: Vec<HealthCheck>, healthy: Option<bool>, machine: Option<TargetReport>,
  source: Option<String>, extra: Map }` — all `bon::Builder`.
- `TargetReport { detail: Map, health: Option<Vec<HealthCheck>> }` — machine and
  application are described alike.
- `ApplicationReport { detail: Map, health: Option<Vec<HealthCheck>>, type_: String }`.
- `ApplicationType(String)` is an open set, so `tamanu-central` / `tamanu-facility`
  are plain strings derived from `ApiServerKind`.
- `source` is transitionally optional and becomes mandatory; absent is attributed to
  `alertd`; the names `canopy` and `manual` are reserved.
- The response carries per-target `check_severities` under `machine` /
  `applications` (`TargetResponse`), keyed by **bare** check name — so a machine
  check and an application check may share a name, each answered under its target.

## Catalogue split, verified against the registry

`SUBJ`'s prose maps onto the 45-check registry with nothing left over: 18 machine, 27
application, every machine name resolving to a real check.

**Machine (18):** `disk_free`, `inodes`, `btrfs`, `held_captures`, `time_sync`,
`memory`, `load`, `uptime`, `external_users`, `ips`, `munin`, `billing_tags`,
`tailscale`, `tailscale_config`, `canopy_registration`, `caddy_version`,
`caddy_resolvers`, `caddyfile_version`.

**Application (27):** everything else — the `db_*`, `fhir_*`, `sync_*` and `*_errors`
families, plus `migrations`, `reporting_roles`, `pg_tuning`, `pg_checksums`,
`tamanu_http`, `tamanu_service`, `version_drift`, `http_errors`, `caddy_certs`.

Two mappings worth noting because the card description does not spell them out:

- **The caddy family splits.** `caddy_version`, `caddy_resolvers` and
  `caddyfile_version` grade the front-end software itself and are machine checks;
  `caddy_certs` is an application check, because the certificates are the
  application's.
- **`pg_tuning` is settled as an application check.** The card left this open; `SUBJ`
  decides it. The machine-memory denominator stays an interim wart here, and K1's
  plan carries the fix (taking the denominator from the Postgres service's declared
  ceiling). Worth a code comment so it is not read as an oversight.

`SUBJ` also lists per-service resource usage as an application check. That is K1's
scope, not this card's, so it stays unimplemented here.

## Facts split, verified against `ServerInfo`

`ServerInfo`'s 26 fields split 18 machine / 8 application, and four further machine
facts arrive via `payload_extras` lifted from machine checks (`munin`, `lanIps`,
`wanIpv4`, `wanIpv6`) — giving the 22 / 8 the card describes.

**Application (8):** `tamanu_version`, `tamanu_server_kind`, `tamanu_root`,
`node_version`, `canonical_url`, `current_sync_tick`, `timezone`, `pg_version`.

**Machine (22):** the remaining 18 `ServerInfo` fields — `bestool_version`,
`hostname`, `os_timezone`, `uptime_secs`, `cpu_cores`, `total_memory_bytes`,
`os_kind`, `os_name`, `os_version`, `kernel`, `arch`, `virtualised`,
`virtualisation`, `filesystems`, `ipv4`, `ipv6`, `nat64`, `instance_tags` — plus the
four lifted extras.

`tamanu_service` lifts a `services` extra that is application-side and moves with it.

The `timezone` / `os_timezone` pair straddles the split by design: Tamanu's configured
zone is the application's, the clock zone is the machine's, and `SUBJ` requires drift
between them to stay gradable wherever one sweep holds both.

## Open questions

1. **How each check declares subject + application scope**, and what becomes of the
   `@tamanu` / `db` / `host` input categories. They encode a different axis (what
   inputs a check needs) and still drive skips, so adding subject as an orthogonal
   field is additive; folding them together is a larger refactor K1 may redo.
2. **Where the split takes shape** — build a typed `StatusPayload` in the sweep, or
   keep a structured `Value` and assemble at the push boundary. The first is the
   cleaner end state but touches every payload consumer (`apply_severities`,
   `overall_from_payload`, CLI render, `endpoint_latest`).
3. **The static application key.** `SUBJ` requires an agent on a machine to use a
   fixed key for the application it reports, stable across pushes, and never reused
   under a different type. The exact string is still to choose.
4. **The machine-id rename.** New name for `get_or_create_server_id`, applied at both
   call sites, while still accepting a `server_id` from canopy registration files.
5. **Severity reconciliation.** Whether A2 also consumes the per-target
   `check_severities` from the response, or leaves that to K1 and keeps reading the
   top-level map.
