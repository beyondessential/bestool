# `bestool tamanu doctor`

## Context

We have no on-server quick-check for a Tamanu deployment. To diagnose a misbehaving install, an operator currently has to manually inspect the config, query the DB, eyeball `systemctl`/`taskmgr`, and pull stats from `htop`/`df`. The `doctor` subcommand collects that information in one command, presents it on the CLI with colour-coded pass/fail/warning indicators, and optionally pushes the structured result to Canopy at `POST /status/{server_id}`, where Canopy's new health-driven incidents pipeline (canopy PR #131) consumes it to drive incidents.

The Canopy contract (per PR #131):
- Body: free-form JSON (uptime, pgVersion, timezone, …) PLUS the reserved keys `healthy: bool` (absent ⇒ `true`) and `health: [{check, healthy, …extras}]`.
- A top-level `healthy: false` opens a roll-up incident (`source="status"`, `ref="health"`).
- Per-check `healthy: false` entries open per-check issues (`source="status"`, `ref="health/{check}"`), Warning while top is healthy, Error otherwise.

## Files to add/modify

### New module: `crates/bestool/src/actions/tamanu/doctor.rs` + `doctor/`

Directory layout — one file per check so adding new checks is mechanical:

```
src/actions/tamanu/doctor.rs           # CLI entry: arg parsing, dispatch, rendering, payload
src/actions/tamanu/doctor/check.rs     # `Check`, `CheckStatus`, common trait
src/actions/tamanu/doctor/server_info.rs  # non-check facts: os, virt, filesystems, ipv4/6/nat64
src/actions/tamanu/doctor/checks/
    mod.rs           # re-exports + registry (name → run fn)
    db_connect.rs
    db_version.rs
    server_id.rs
    migrations.rs
    disk_free.rs
    memory.rs
    load.rs
    uptime.rs
    time_sync.rs
    tamanu_http.rs
    tailscale.rs     # CLI-only; not in health[]
    tamanu_service.rs
    sync_sessions.rs
    fhir_jobs.rs
    http_errors.rs   # deferred if no schema match
```

Public surface (just what `tamanu.rs` calls):
```rust
#[derive(Debug, Clone, Parser)]
pub struct DoctorArgs { /* see CLI flags below */ }
pub async fn run(ctx: Context<TamanuArgs, DoctorArgs>) -> Result<()>;
```

Internal structure:
- `Check` struct: `name: &'static str`, `status: CheckStatus`, `summary: String`, `details: serde_json::Value` (extra fields the check wants to surface).
- `CheckStatus` enum: `Pass`, `Warning(String)`, `Fail(String)`.
- One async fn per check, each returning a `Check`. Run them concurrently with `futures::join!` or `tokio::join!` where independent.
- `render(checks, server_info, use_colours, out: &mut impl Write)`: ANSI rendering to stdout.
- `build_payload(checks, server_info)`: produces the JSON that gets POSTed.
- `post_to_canopy(client, server_id, payload)`: the wire layer.

### CLI flags on `DoctorArgs`

- `--send` — POST result to Canopy after rendering locally. Default off.
- `--canopy-url <URL>` — override base URL, default `DEFAULT_CANOPY_URL` (`https://meta.tamanu.app`).
- `--json` — emit the JSON payload to stdout instead of the human render. Useful for piping and for testing what would be sent.
- `--no-colour` — force colour off (otherwise inherits `TamanuArgs::use_colours`, which is already wired from the top-level logging colour setting).
- `--check <name>` (repeatable) — run only the named checks. Default: all.

The `--root` flag is already on `TamanuArgs` (the parent) and is inherited.

### Modifying `crates/bestool/src/actions/tamanu.rs`

Register the new subcommand in the `subcommands!` macro block (around line 70):
```rust
#[cfg(feature = "tamanu-doctor")]
#[clap(alias = "doc")]
doctor => Doctor(DoctorArgs),
```

### Modifying `crates/bestool/Cargo.toml`

Add a new feature gating the subcommand:
```toml
tamanu-doctor = [
    "__tamanu",
    "tamanu-config",
    "dep:bestool-alertd",      # CanopyClient + types
    "dep:bestool-psql",        # DB pool
    "dep:p256",                # device key handling, for canopy mTLS auth
    "dep:sysinfo",             # system metrics (CPU, RAM, disks, OS)
    "dep:tokio-postgres",
    "dep:duct",                # invoking systemctl / pm2 / systemd-detect-virt / tailscale
    "dep:hickory-resolver",    # NAT64 AAAA probe
]
```

Add `owo-colors = "4"` as a new optional dep and gate it under `tamanu-doctor` too.

And add `tamanu-doctor` to the `tamanu` umbrella feature.

### Extracting `get_or_create_server_id`

Currently `pub(super)`-equivalent (it's a free fn in `meta_ticket.rs`). Make it `pub` and reuse it from `doctor.rs`:

```rust
// meta_ticket.rs — drop the private `async fn`, make it
pub async fn get_or_create_server_id(client: &tokio_postgres::Client) -> Result<String>
```

This avoids duplicating the SELECT/INSERT-from-`local_system_facts` logic.

### Adding `post_status` to `CanopyClient`

The existing `CanopyClient::post_event` is for `/events` and posts a typed `NewEvent`. The `/status` route takes free-form JSON. The cleanest split is to add a sibling method that mirrors `post_event` but POSTs to a different path:

```rust
// crates/alertd/src/canopy.rs
pub async fn post_status(
    &self,
    base_url: &Url,
    server_id: &str,
    payload: &serde_json::Value,
) -> Result<()>
```

URL routing matches the existing pattern: in tailscale mode, `{TAILSCALE_URL}/public/status/{server_id}`; in mTLS mode, `{base_url}/status/{server_id}`. (`base_url` is whatever the operator passed via `--canopy-url`.) This keeps the auth-path-selection logic in one place and is the minimal change. **No new crate split needed** — TODO.txt mentions it as "might be worth", but extracting now would be premature for a single second caller.

## Checks to implement

These map to one "health check" each. Both the CLI display and the `health[]` array entry have the same `check` name.

| name            | what it does | extras in JSON |
|---|---|---|
| `tamanu_found`  | A Tamanu install was discovered via `find_tamanu`. Returns the version + root path. **Fail** if none found (everything else short-circuits). | `version`, `root`, `kind` (central/facility) |
| `db_connect`    | Open a `tokio_postgres` connection from `ConnectionUrlBuilder`. **Fail** if connect errors. | `db_host`, `db_name`, `latency_ms` |
| `db_version`    | `SELECT version()` after connect. Pass-through. | `pg_version` |
| `server_id`     | Look up `metaServerId` in `local_system_facts`. **Pass** when present; **Pass** *and* creates one when absent (logged at info level — match existing `get_or_create_server_id` semantics). | `server_id` |
| `migrations`    | `SELECT max(timestamp) FROM "SequelizeMeta"` (or whichever migrations table Tamanu uses; verify against a live DB before committing). **Warning** when stale by some threshold — defer threshold to a future tweak; for first pass, just surface the most recent migration name. | `last_migration` |
| `disk_free`     | Free space on the partition holding the Tamanu root and on `/` (Linux) / `C:` (Windows). **Warning** at <20%, **Fail** at <5%. Use `sysinfo::Disks`. | `mountpoint`, `free_bytes`, `total_bytes`, `percent_used` per relevant mount |
| `memory`        | Free RAM via `sysinfo::System`. **Warning** at <10% free, **Fail** at <2%. | `used_bytes`, `total_bytes`, `percent_used` |
| `load`          | Linux load average via `sysinfo::System::load_average()` (skipped on Windows with a notice). Don't fail on raw numbers; just report. | `one_min`, `five_min`, `fifteen_min` |
| `uptime`        | `sysinfo::System::uptime()`. Pass-through. | `uptime_secs` |
| `time_sync`     | Linux: `timedatectl show -p NTPSynchronized --value` if available — Pass if "yes". Skipped (info-only) elsewhere. | `synchronized`, `service` |
| `tamanu_http`   | GET `http://localhost/api/public/ping` (basic liveness). Pass on 2xx, Fail otherwise. 5s timeout. | `url`, `status_code`, `latency_ms` |
| `tailscale`     | `tailscale status --json` parse. Pass if present and Self.Online, Warning if installed but offline, **info-only** (no warning) if not installed. CLI-rendering only — **not** emitted into `health[]` because canopy already tracks tailscale identity elsewhere. | `ip`, `name`, `online` |
| `tamanu_service`| Whether Tamanu's process supervisor reports the API server up. Linux: `systemctl is-active tamanu-central` / `tamanu-facility` via `duct` or `Command`. Windows: `pm2 jlist` (parse JSON) and look for the central/facility process — pm2 ships as part of the standard BES Windows deployment. **Fail** if absent or stopped. | `supervisor` (`systemd`/`pm2`), `unit_or_process_name`, `state` |
| `sync_sessions` | Query `SELECT count(*) FROM sync_sessions WHERE completed_at IS NULL AND started_at < now() - interval '1 hour'` (verify column/table names against the current Tamanu schema before committing — this is a known table family but exact column names should be confirmed). **Warning** if any session is older than 1h, **Fail** if older than 6h. Also report total active count. | `active_count`, `stuck_count`, `oldest_started_at` |
| `fhir_jobs`     | `SELECT status, count(*) FROM fhir.jobs GROUP BY status`. Surface the per-status counts; **Warning** if `errored` count is non-zero and >5% of total; **Fail** at >50% (thresholds tunable as constants). | `total`, `by_status: {queued, in_progress, errored, …}` |
| `http_errors`   | HTTP error rate over a recent window. Tamanu doesn't necessarily expose a Prometheus metric directly — most reliable source is a DB-side count of recent failed requests if Tamanu logs them (check `audit_log` / `request_log` style tables; if none exists, this check is a no-op and reports `skipped` rather than Pass). **Warning** if >1% errors over last 5 min, **Fail** at >10%. Threshold tunable. **Verify the right query against a live DB before committing**; if no suitable table, drop the check from the first cut and leave a TODO. | `window_secs`, `total_requests`, `error_requests`, `error_rate_pct` |

The list is open-ended — the TODO is loose on which checks to include. The above is a reasonable starter set covering "is Tamanu installed / running / talking to its DB / hosted on a healthy machine".

A check that returns `Fail` flips top-level `healthy` to false. `Warning` does not — it lands in `health[]` as `healthy: false` (a non-fatal per-check failure, which is legal in the canopy contract) but the top-level stays `healthy: true`. The wire format mapping is detailed in "Canopy payload" below.

## Output rendering

A single-line summary at the bottom is printed to **stdout** (`println!`). All per-check lines also go to stdout. Errors talking to Canopy go to **stderr**. That matches the TODO ("Print a single line to STDOUT for the overall health and STDERR any errors when sending to Canopy").

Format (column-aligned with the check name):

```
Tamanu doctor (server-id: 8a1f…b042)

  PASS    tamanu_found   Tamanu 2.15.1 at /opt/tamanu (central)
  PASS    db_connect     postgres@localhost:5432 (3ms)
  PASS    db_version     PostgreSQL 14.10
  PASS    server_id      stored in local_system_facts
  WARN    device_key     not present — canopy mTLS unavailable
  PASS    migrations     last: 20251021-add-fhir-coverage
  FAIL    disk_free      / 96% used (12GB of 256GB free)
  PASS    memory         48% used
  PASS    uptime         5d 12h
  …

Result: FAILING (1 failed, 1 warning)
```

The final result line is tri-state, mirroring the per-check states:
- **HEALTHY** — all checks Pass.
- **DEGRADED** — at least one Warning, zero Fails. (Top-level `healthy: true` on the wire; canopy will surface per-check Warning issues but not open an incident from them alone.)
- **FAILING** — at least one Fail. (Top-level `healthy: false` on the wire; canopy opens an incident.)

The single stdout summary line (the one promised in the TODO) is this result line — short, parseable, suitable for being grep'd by a wrapper script.

Colours (via `owo-colors`, which is already a transitive dep of miette):
- PASS — green
- WARN — yellow
- FAIL — red
- Result line: green for HEALTHY, yellow for DEGRADED, red for FAILING.

Use `owo_colors::OwoColorize` with `if_supports_color` so colours auto-disable when stdout is not a tty, with the `--no-colour` flag forcing off and the existing `TamanuArgs::use_colours` (already populated from the top-level `--color` / `NO_COLOR` plumbing) as the override path.

**Add `owo-colors = "4"`** to `bestool/Cargo.toml` as a non-optional dep gated by the doctor feature, or to the workspace if other commands could use it later. Default to feature-gated to keep the doctor opt-in clean.

## Canopy payload

Wire shape, matching the canopy PR #131 contract:

```jsonc
{
  // free-form section
  "bestool_version": "1.7.1",
  "tamanu_version": "2.15.1",
  "hostname": "tamanu-prod-01",
  "canonical_url": "https://central.example.com",
  "uptime_secs": 482910,
  "pg_version": "PostgreSQL 14.10 ...",
  "timezone": "Pacific/Auckland",              // from jiff::tz::TimeZone::system()

  // OS / platform — sourced directly from sysinfo + /etc/os-release on Linux,
  // GetVersionEx + registry on Windows
  "os_kind": "linux",                          // linux | windows | macos
  "os_name": "Ubuntu",
  "os_version": "22.04.4 LTS",
  "kernel": "6.5.0-35-generic",
  "arch": "x86_64",

  // virtualisation — from systemd-detect-virt on Linux, WMI Win32_ComputerSystem.Model on Windows
  "virtualised": true,
  "virtualisation": "kvm",                     // null / "none" when bare metal

  // filesystem type for each disk reported below
  // (mountpoint-keyed so it can be cross-referenced with disk_free entries)
  "filesystems": [
    {"mountpoint": "/", "fs_type": "ext4"},
    {"mountpoint": "/var/lib/postgresql", "fs_type": "ext4"}
  ],

  // network connectivity — three independent probes:
  // - ipv4: can resolve+TCP-connect to a known IPv4 endpoint (e.g. 1.1.1.1:443)
  // - ipv6: same but for IPv6 (2606:4700:4700::1111:443)
  // - nat64: DNS AAAA lookup of a known IPv4-only name through the system resolver returns
  //   a synthesised v6 (well-known `ipv4only.arpa` or similar)
  "ipv4": true,
  "ipv6": false,
  "nat64": false,

  // reserved keys (per canopy PR #131 contract)
  "healthy": false,
  "health": [
    {"check": "db_connect",    "healthy": true,  "latency_ms": 3},
    {"check": "disk_free",     "healthy": false, "mountpoint": "/", "percent_used": 96, "free_bytes": 12884901888, "total_bytes": 274877906944},
    {"check": "migrations",    "healthy": true,  "last_migration": "20251021-add-fhir-coverage"},
    {"check": "tamanu_service","healthy": true,  "supervisor": "systemd", "unit_or_process_name": "tamanu-central", "state": "active"},
    {"check": "sync_sessions", "healthy": false, "active_count": 3, "stuck_count": 1, "oldest_started_at": "2026-05-13T01:14:02Z"},
    {"check": "fhir_jobs",     "healthy": true,  "total": 1245, "by_status": {"queued": 12, "in_progress": 3, "errored": 0, "completed": 1230}}
    // ... one entry per executed check
  ]
}
```

Notes on the new top-level fields:
- `os_kind` / `os_name` / `os_version` / `kernel` come from `sysinfo::System::name()` / `os_version()` / `kernel_version()` / `long_os_version()`. The user noted the OS used to be extracted from the PG version string — this replaces that approach with a direct, server-side source.
- `virtualised` + `virtualisation` come from `detect_virtualisation()` in `meta_ticket.rs` (already implemented for Linux; Windows side needs a small addition — easiest is `wmic computersystem get Model` or reading the BIOS info).
- `filesystems` reuses `sysinfo::Disks` (same call as `disk_free`); take `disk.file_system()` per mount.
- `ipv4` / `ipv6` / `nat64` probes:
  - `ipv4`: open a TCP connection to `1.1.1.1:443` with a 3s timeout.
  - `ipv6`: open a TCP connection to `[2606:4700:4700::1111]:443` with a 3s timeout.
  - `nat64`: AAAA resolve of `ipv4only.arpa` via the system resolver (uses `hickory-resolver`, already in the workspace dep tree behind the `download` feature — pull it in directly here too).
  - Each probe is its own ~30 line function; results are booleans, not Pass/Fail checks (these are *capability* facts, not healthchecks).

Per-check `healthy` mapping (per canopy contract — non-fatal failures are *legal* with top-level still healthy):
- **Pass** ⇒ `healthy: true`. Does not affect top-level.
- **Warning** ⇒ `healthy: false`. Top-level **stays** `healthy: true`. Canopy will file a per-check issue at Warning severity (joins but doesn't open an incident).
- **Fail** ⇒ `healthy: false`. Flips top-level to `healthy: false`. Canopy will file the per-check issue at Error severity AND open the roll-up incident.

Tamanu-side rule, then: top-level `healthy: false` iff **any** check is Fail. Warnings alone keep the top healthy.

Notably absent from the wire payload (canopy already has these from the meta-ticket / reachability path):
- `tamanu_kind` (central/facility)
- `tamanu_root` (path on disk)
- `tailscale_ip`, `tailscale_name`
- `hosting` (ec2/iti) — meta-ticket already reports this. The virtualisation tech is now reported instead (`virtualisation`) which is the more useful signal for "what shape of host am I on".

## Reuse map

- `find_tamanu()` — `tamanu.rs:99`
- `load_config()` — `tamanu/config.rs`
- `ConnectionUrlBuilder` — `tamanu/connection_url.rs:16`
- `bestool_psql::create_pool()` — exact pattern from `meta_ticket.rs:61`
- `get_or_create_server_id()` — `meta_ticket.rs:106` (will be made pub)
- `get_tailscale_info()` — `meta_ticket.rs:146` (will be made pub or moved to a shared module)
- `detect_virtualisation()` — `meta_ticket.rs:244` (will be made pub or moved); `is_raspberry_pi()` / `is_ec2()` stay private to meta_ticket since doctor surfaces the raw virtualisation string instead.
- `fetch_device_key()` — `tamanu/alertd.rs:358` (already async; make it `pub` so doctor can reuse the exact "best-effort, log-and-return-None" semantics)
- `CanopyClient::new()` — `alertd/src/canopy.rs:112`
- `DEFAULT_CANOPY_URL`, `TAILSCALE_URL` — `alertd/src/canopy.rs:14, 20`

The `meta_ticket.rs` helpers should be moved to a small `tamanu/server_info.rs` module so both meta-ticket and doctor pull from one place. This is a tiny refactor (cut a handful of fns and adjust imports) and avoids the `pub(super)` smell of reaching into a sibling subcommand. Same for `fetch_device_key` from `alertd.rs` — promote it to the shared module.

## Verification

End-to-end:
1. Build with the feature: `cargo build -p bestool --features tamanu-doctor`.
2. With a local Tamanu install + DB up: `DATABASE_URL=postgresql://localhost/tamanu_meta cargo run -p bestool -- tamanu doctor` — should render the table to stdout, no Canopy traffic.
3. `… tamanu doctor --json` — pipe to `jq`, verify the payload matches the wire shape above (in particular `healthy` and `health[]` keys, the OS/virt/filesystems/v4/v6/nat64 blocks, and the new per-check entries for `tamanu_service`, `sync_sessions`, `fhir_jobs`).
4. `… tamanu doctor --send --canopy-url https://canopy-staging.example` against a staging canopy that has PR #131 deployed — verify the row lands and a Warning issue files for a per-check Warning, and an incident opens for a Fail.
5. Without a `deviceKey` in `local_system_facts` and without tailscale, `--send` should print to stderr that canopy is unreachable, and exit non-zero.
6. With colour piped through `less -R` (or `--color always` equivalent), ANSI codes render correctly.
7. On Windows in a dev VM: confirm `tamanu_service` finds the pm2 process (it expects pm2 to be on PATH; same approach the existing BES Windows tooling uses). On Linux: confirm `systemctl is-active tamanu-central` / `tamanu-facility` resolves the right unit.
8. Trigger DEGRADED (run with one Warning, zero Fails) and FAILING (one Fail) — confirm summary line + colour matches.

Tests (in `doctor.rs`'s `#[cfg(test)] mod tests`):
- A check-result struct round-trips through `build_payload` correctly (Pass / Warning / Fail → `healthy` bools).
- `build_payload` sets top-level `healthy: false` iff any check is Fail.
- `render` produces colourless output when colours are off; produces correct count summary at the bottom.
- (Following AGENTS.md DB rule) DB-touching helpers go through the existing `local_system_facts` test pattern: same env var `DATABASE_URL=postgresql://localhost/tamanu_meta`, same setup as `meta_ticket.rs` tests.

## Out of scope (call out, don't implement)

- Splitting the canopy client into its own crate (mentioned as a "might" in the TODO). Defer — adding `post_status` to the existing `alertd::canopy` module keeps the diff small. Worth revisiting if a third caller appears or if `bestool` ever needs to use the canopy client without depending on the rest of `bestool-alertd`.
- Configurable thresholds (disk %, memory %). Hardcoded for first pass; surface as constants near the top of the file.
- Automatic scheduling (cron / systemd timer / windows task) of `--send`. The TODO doesn't ask for it; can be wired as a separate command later or by the operator with their preferred scheduler.
