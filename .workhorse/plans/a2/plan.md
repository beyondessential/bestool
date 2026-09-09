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

## The two axes are genuinely different, so both stay

The `entry!` macro's categories say **what inputs a check needs**, and gate whether it
runs at all:

- **`@tamanu`** (default arm, 22 checks) — needs a Tamanu deployment. Runs only when
  the sweep has a Tamanu context *and* it is really Tamanu's; otherwise skips with
  "no Tamanu on this host". Receives the unwrapped `CheckContext`.
- **`db`** (4 checks) — needs any database, Tamanu's or the generic `DATABASE_URL`
  fallback. Skips only when there is no database at all. Also receives `CheckContext`.
- **`host`** (19 checks) — needs neither, runs unconditionally, and receives the whole
  `SweepContext`.
- **`off_wire`** modifies any of the three: the check renders in the CLI but stays out
  of the wire `health[]`.

Cross-tabulating category against `SUBJ`'s subject shows the axes nearly coincide but
cross in three places, which is what proves they cannot be collapsed into one:

| | machine | application |
|---|---|---|
| `host` | 17 | **2** (`caddy_certs`, `http_errors`) |
| `@tamanu` | **1** (`caddyfile_version`) | 21 |
| `db` | 0 | 4 |

`caddyfile_version` is the sharpest illustration: it is a **machine** check by subject
(it grades the front-end's configuration marker) yet it needs the **application's**
version to decide whether that marker is outdated. Whose subject a check speaks for
and what it must read to speak are independent, so subject and scope are added as new
declarations and the categories keep their present job.

## Decisions

**The sweep is typed, and a check's identity is scoped.** A check is identified by its
subject together with its name, not by name alone: a machine `check_a`, a
`tamanu-central` `check_a` and a `tamanu-facility` `check_a` are three different
checks. This is what the wire already assumes — Canopy keys `check_severities` per
target under bare check names — so a flat name list cannot represent it. The sweep
therefore builds the typed `StatusPayload` directly and `SweepResult.payload: Value`
retires.

**Scope is declared per check, not held in a central table.** Each registry entry
states its subject and, for an application check, which application types it applies
to, so a check keeps owning everything about its own function. This hoists the
`kind` gating that 13 checks currently do inline — every one of them central-only,
each opening `run()` with a skip when the kind is not Central. (`fhir_jobs` gates only
its heal action, not its run, and is unaffected.)

**The split response is consumed.** `apply_severities` reads the per-target
`check_severities` the push returns — the machine's map for machine checks, each
application's for its own — rather than one flat map. The `ABSENT_CHECK_SEVERITY`
warn default still applies, looked up in the right target's map.

**A check outside its subject's scope is omitted, not skipped.** It does not run and
does not appear on the wire or in the render, so a facility stops reporting the 13
central-only checks rather than reporting them as skipped. Canopy recovers a check
that stops being reported, so no issue is left hanging. Scope decides applicability;
`skip` keeps its existing meaning of "applicable, but could not be determined".

**Check selection is always scope-qualified.** `--check` and `--skip` take
`machine:disk_free` or `tamanu-central:migrations`; a bare name is an error rather
than a wildcard, because a bare name cannot say which of several same-named checks is
wanted. The error names the qualified forms that exist for what was typed, so a bare
`--check disk_free` answers with `machine:disk_free`. Unknown-name validation, fatal
today, becomes scope-aware on the same terms.

**The application key is the substrate prefix and the type: `host-tamanu-central`.**
The prefix is the literal `host` for now and comes from the substrate once K1 lands.
The format is chosen to read well on the wire, where a self-describing key is far
easier to debug than an opaque one. Nothing else rests on it: Canopy handles key and
type correlation itself, so the key needs only to be fixed and stable, which any
format would satisfy.

**`get_or_create_server_id` becomes `get_or_create_machine_id`**, with its doc stating
that this is the Canopy machine identity and *not* the OS `/etc/machine-id`. That
warning earns its place: `crates/canopy/src/registration.rs:399` already reads the OS
machine id via the `machine-uid` crate and calls it "the host machine id", so the two
sit in one codebase. `machine_id` is also already Canopy's wire vocabulary, so the
rename moves toward the schema rather than away from it.

The rename carries `standard_server_id_path`, the `_at` test shim, the file
read/write helpers, and the `metaServerId` wording in the log and error messages. Two
things deliberately keep the old name: the canopy registration file's own `server_id`
field, which is an on-disk format bestool must keep reading, and the `server_id` path
parameter on the status endpoint.

## What the typed sweep touches

- `SweepResult` — `results: Vec<(Check, bool)>` gains the subject; `payload: Value`
  becomes the typed payload; `server_id` becomes the machine id.
- `build_payload` — routes each check into its subject's `health[]`, and each lifted
  `payload_extra` into its subject's `detail`: `munin`, `lanIps`, `wanIpv4`, `wanIpv6`
  to the machine, `services` to the application.
- `overall_from_payload` — must union the machine's health with every application's,
  instead of reading one `health` array.
- `apply_severities` / `severity_ceiling` — per-target lookup, as above.
- `task.rs` — builds the payload directly instead of
  `serde_json::from_value(sweep.payload)`, and sets `source` to `alertd`.
- CLI render and TUI — the sort key becomes subject-then-name, and the subject has to
  be visible wherever two scopes share a name.
- `endpoint_latest` and the CLI's `results_from_wire` — the daemon caches a sweep and
  the CLI parses it back out of `payload["health"]`, so both ends of that round-trip
  learn the split shape together.
- Per-scope name uniqueness is now the invariant to hold (a flat unique-name list no
  longer expresses it), so it wants asserting in a test.

## Open questions

None outstanding: the design decisions above cover the card's scope. Two things are
known and deliberately left as they are.

**`pg_tuning` reads the machine's total memory for its denominator** while being an
application check. `SUBJ` settles it as an application check, and the reading stays
until K1 takes the denominator from the Postgres service's declared ceiling. It wants
a code comment so it is not read as an oversight.

**The daemon's cached-sweep round-trip is a matched pair.** The daemon caches a sweep
and the CLI parses it back, so `endpoint_latest` and `results_from_wire` change
together. A CLI and a daemon of different versions on one host will disagree about the
payload shape for as long as they are mismatched.
