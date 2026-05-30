# Plan: consolidate healthchecks into alertd, migrate YAML alerts to checks, retire the alert engine (TODO #10)

## Context

Two monitoring systems run in parallel:

- **Healthchecks** (doctor): code-defined `Check`s run as a concurrent "sweep" → pass/warning/fail + JSON details, POSTed to **canopy** (`POST /status/{server_id}`). Today these live in `crates/tamanu/src/doctor/` (the `Check` type + checks), with the sweep orchestration (`perform_sweep`), canopy posting, and the `DoctorTask` background task in `crates/bestool/`. Viewable via `bestool tamanu doctor`.
- **alertd** (`crates/alertd/`): a daemon loading **YAML alert definitions** (deployed in Tamanu installs at `/etc/tamanu/alerts`, …), scheduling each on its own interval, evaluating SQL/shell/event sources, and dispatching to email/Slack/canopy `/events`. Ships a standalone `bestool-alertd` binary + library; also hosts the `DoctorTask`.

Decisions:

1. **Invert the crate relationship.** Move the whole doctor subsystem (framework + checks + sweep + canopy posting + `DoctorTask`) **into `bestool-alertd`**, which calls into `bestool-tamanu` for common Tamanu domain utilities. alertd becomes the monitoring engine that owns both the framework and the checks. No dependency cycle: `bestool-tamanu` never depends on alertd.
2. **Migrate** all 16 production YAML alerts (`~/code/work/tamanu/alerts`) into checks. Migrated checks default to **`Check::fail`** when triggered (single severity, no warn tier).
3. **Canopy owns alerting** and has its own logic — it ignores the sweep's top-level `healthy:false`, so the warn-vs-fail-for-top-level distinction is irrelevant at the canopy level. bestool just posts the sweep; drop email/Slack/per-alert targets, dedup, hysteresis, cadence.
4. **Retire the YAML alert engine and the standalone CLI**; alertd keeps only the daemon framework + the doctor subsystem.
5. **Then review thresholds** across all checks (migrated and pre-existing).

Note: deployed installs still have YAML files under `/etc/tamanu/alerts`; once the loader is removed they're simply ignored (no error). Operators can delete them later.

## Target architecture

- **`bestool-alertd`**: owns the monitoring framework (`BackgroundTask` daemon, http server) **and** the doctor subsystem — `Check`/`CheckStatus`/`OverallResult` wire types, `CheckContext`, the registry + `checks/*`, `progress`, the `ServerInfo` facts, `perform_sweep` + `SweepResult` + canopy status posting, and a built-in `DoctorTask` it registers itself. Depends on `bestool-tamanu` (common domain), `bestool-canopy`, `bestool-postgres`, `bestool-kopia`.
- **`bestool-tamanu`**: common Tamanu domain library only — `config`, `roots`, `connection_url`, `services`, `systemd`, `pm2`, `server_info` (DB queries: metaServerId, patient-portal), `versions`, `ApiServerKind`, `find_tamanu`, `detect_kind`. The `doctor` module and `doctor` feature are removed; description updated.
- **`bestool`**: thin CLI. `bestool tamanu doctor` keeps arg parsing + human rendering + daemon-fetch (`/tasks/doctor/latest`/`recompute`) and calls `bestool_alertd::doctor` for local sweeps + types. `bestool tamanu alertd` configures and runs the alertd daemon (which self-registers its `DoctorTask`).

## Phase 1 — Invert: move the doctor subsystem into alertd (behaviour-preserving refactor)

- Relocate `crates/tamanu/src/doctor/{check,checks,checks/*,progress,server_info}.rs` → `crates/alertd/src/doctor/…`.
- Move `perform_sweep` + `SweepResult` + canopy status posting from `crates/bestool/src/actions/tamanu/doctor.rs` into alertd (e.g. `bestool_alertd::doctor::perform_sweep`).
- Move `DoctorTask` (`crates/bestool/src/actions/tamanu/alertd/doctor_task.rs`) into alertd as the built-in task; alertd registers it (or exposes a constructor) so bestool no longer wires it.
- Add `bestool-tamanu` as an alertd dependency; rewrite check imports from `crate::{ApiServerKind, config::TamanuConfig, services, systemd, pm2, server_info, detect_kind, versions}` → `bestool_tamanu::{…}`.
- Move doctor-only deps (`bestool-kopia`, `hickory-resolver`, and `reqwest`/`owo-colors` as needed) from `crates/tamanu/Cargo.toml` to `crates/alertd/Cargo.toml`; remove tamanu's `doctor` feature and update its package description.
- bestool side: `doctor.rs` keeps CLI args + rendering + daemon-fetch, calling `bestool_alertd::doctor`; delete the moved `doctor_task` module; retarget Cargo features (`bestool-tamanu/doctor` → alertd).
- **Behaviour-preserving** — no check logic changes. This is large but mechanical (mostly imports + module moves).

## Phase 2 — Migrate the 16 YAML alerts to checks (now in alertd), default FAIL, central-only

Migrated checks emit `Check::fail` when triggered, skip on Facility (gate on `ctx.kind`, mirroring `fhir_jobs`), and attach offending rows as `details`. A shared "recent error rows" helper serves the 7 recent-error alerts (run query → `fail` with rows if any match, else `pass`); the old per-alert `$1 = now - interval` becomes a per-check lookback constant. Verbatim SQL is in `~/code/work/tamanu/alerts/<name>.yml`.

**New checks (~10):**
| Alert(s) | New check | Style |
|---|---|---|
| certificate-notification-error | `certificate_notification_errors` | recent-error |
| ips-error | `ips_errors` | recent-error |
| patient-communications-error | `patient_communication_errors` | recent-error |
| report-error | `report_errors` | recent-error |
| fhir-error | `fhir_job_errors` | recent-error |
| sync-errors-mobile + sync-errors-server | `sync_session_errors` (one check; detail splits mobile/server; keep benign-error exclusions) | recent-error |
| sync-facility-not-syncing + sync-no-sessions | `sync_facility_stale` (one check; facilities with no recent successful sync) | stuck |
| sync-lookup-stale | `sync_lookup` (**= TODO #8**) | stuck |
| sync-restart-loop | `sync_restart_loop` | threshold |
| fhir-unresolvable-service-requests-labs | `fhir_service_requests_unresolved` | stuck |

**Already covered (confirm/extend detail, no new check):** fhir-queue-incredibly-large, fhir-queued-job-long, fhir-running-job-long → `fhir_jobs`; sync-long → `sync_sessions`.

Add via the registry pattern: `pub mod <name>;` + `entry!("<name>", <name>)` in the registry; `pub async fn run(ctx: CheckContext) -> Check`. Split into ~3 PRs by theme (error-notification / sync / fhir+reconcile).

## Phase 3 — Retire the YAML alert engine + standalone CLI

- Remove from alertd: `alert.rs`, `loader.rs`, `glob_resolver.rs`, `events.rs`, `targets.rs` + `targets/*`, `templates.rs`, per-alert `state_file.rs`, the alert parts of `scheduler.rs`, `commands.rs` + `commands/*`, `main.rs`, the `[[bin]]` + `cli` feature, `windows_service.rs`. Trim `DaemonConfig` (drop `alert_globs`, `email`, `server_kind`, alert `dry_run`; keep `pg_pool`, `database_url`, `device_key_pem`, `tamanu_version`, `no_server`, `server_addrs`, `watchdog_timeout`, `background_tasks`), `daemon.rs`, `http_server` (drop `/alerts`,`/targets`,`/validate`,`/reload`,`/pause`; keep `/`,`/status`,`/health`,`/metrics`,`/tasks/*`), and `lib.rs` exports. Relocate `InternalContext` out of `alert.rs` into `daemon.rs`/`context.rs`, slimmed to `{ pg_pool, http_client, canopy_client }`.
- bestool: simplify `tamanu alertd` (drop alert-dir discovery/globs, email/Mailgun flags, alert-filtering `server_kind`, and the passthrough subcommands `status`/`reload`/`pause`/`validate`/`loaded-alerts`); keep pg pool, device-key fetch (canopy auth), `tamanu_version`, build `DaemonConfig`, run. Remove the legacy `bestool tamanu alerts` command + module. Delete example alerts (`alerts/`) and alert test fixtures (`crates/bestool/tests/cmd/alerts*`).
- Gated after Phase 2 so coverage isn't lost. Optional follow-up (not in scope): rename `bestool-alertd` / `bestool tamanu alertd` now that it owns healthchecks, not alerts — deferred to avoid crates.io + systemd/install churn.

## Phase 4 — Threshold review (all checks)

After migration, review every check (the 10 migrated + the pre-existing ones) for triggering behaviour: warn-vs-fail, threshold values, central/facility gating, and whether any migrated check should be a warning rather than fail. Produce a short follow-up (possibly its own plan) and adjust. Migrated checks land at FAIL in Phase 2; this pass tunes them.

## Verification

- **Phase 1 (refactor)**: `cargo build`/`clippy` across the workspace and all feature combos; `cargo check -p bestool --target x86_64-pc-windows-gnu`; confirm identical behaviour — `bestool tamanu doctor` (local `--no-daemon` and daemon-fetch `--fresh`), canopy `/status` posting, and `/tasks/doctor/{latest,recompute}` all work; grep for dangling `bestool_tamanu::doctor` references.
- **Phase 2 (checks)**: against the local `tamanu-central` / `tamanu-facility` databases, `cargo test -p bestool-alertd` (DB-backed tests where feasible) and `bestool tamanu doctor --json --no-daemon`; confirm each new check appears as pass/fail with `details`, and is skipped on a facility install.
- **Phase 3 (teardown)**: full-workspace `cargo build`/`clippy` + Windows cross-check (windows_service removed); `bestool tamanu alertd` starts, ticks the sweep, posts to canopy, and `bestool tamanu doctor` still fetches from it; grep for leftover references (`loader`, `targets`, `templates`, `AlertDefinition`, `tamanu alerts`).
