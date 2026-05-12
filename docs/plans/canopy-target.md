# Canopy target for alertd and legacy alerts

## Context

The "canopy" service (open API at https://meta.tamanu.app) exposes `POST /events`
for devices to push event notifications, which canopy aggregates into deduplicated
issues. Bestool currently has two alert pipelines — the new `bestool-alertd` daemon
(`crates/alertd`) and the deprecated `bestool tamanu alerts` cron command
(`crates/bestool/src/actions/tamanu/alerts/`) — and both should be able to forward
alerts to canopy alongside email / slack / zendesk targets.

Authentication is real mTLS: the canopy edge proxy terminates a TLS handshake
that includes a client certificate, then forwards the cert to canopy via
`x-forwarded-client-cert`. Canopy looks up the device by the cert's
SubjectPublicKeyInfo. The Tamanu JS reference (`SendStatusToMetaServer.js`)
uses `undici.Agent({ connect: { cert, key } })` for real mTLS. Documented
header-fallbacks (`mtls-certificate`, `ssl-client-cert`) won't work for external
clients — that's a docs bug being fixed upstream.

The signing material is the same `deviceKey` already used by the existing
`bestool tamanu meta-ticket` flow: a P-256 ECDSA PKCS8 PEM private key in the
Tamanu DB at `SELECT value FROM local_system_facts WHERE key = 'deviceKey'`. We
generate a self-signed X.509 cert from it just-in-time (the mushi pattern in
`/home/felix/code/rust/mushi/lib/src/key.rs`) and use it as the reqwest client
identity.

## High-level design

- Workspace `reqwest` switches from native-tls to `rustls-tls-native-roots`,
  because `Identity::from_pem` requires `rustls-tls`. No existing call sites
  use TLS-implementation-specific APIs (`Client::new()` / `Client::builder()`
  only, no `use_native_tls`).
- New `canopy` module duplicated into both `crates/alertd/src/canopy.rs` and
  `crates/bestool/src/actions/tamanu/alerts/canopy.rs` — small (~80 lines),
  duplication accepted to keep the dep graph clean.
- DeviceKey PEM is consumed at startup; the built `reqwest::Client` is stored
  on the daemon's `InternalContext`. The PEM itself isn't kept long-term to
  avoid accidental Debug-leak.
- `bestool-alertd` (standalone) takes `--device-key-file <PATH>`.
- `bestool tamanu alertd` (wrapper) and `bestool tamanu alerts` (legacy) both
  auto-fetch deviceKey from `local_system_facts`.
- Default event `ref` is `{hostname}/{alert-stem}:{target-id}` — no override
  mechanism, per user direction.
- Alert clearance (`active: false`) **is** supported: canopy targets get
  both trigger and clear dispatches; other target types ignore clear.

## Workspace + deps

- `Cargo.toml` (workspace): change reqwest line to
  `reqwest = { version = "0.13.3", default-features = false, features = ["json", "rustls-tls-native-roots"] }`.
- `Cargo.toml` (workspace): add `rcgen = "0.14.8"` to workspace deps.
- `crates/alertd/Cargo.toml`: add `rcgen = { workspace = true }`.
- `crates/bestool/Cargo.toml`: add `rcgen = { workspace = true, optional = true }`,
  and add `rcgen` + `p256` (already present, but currently gated under
  `tamanu-meta-ticket`) to the `tamanu-alerts` feature.

## Shared canopy module (duplicated)

Each copy exposes:

```rust
pub struct CanopyClient {
    http: reqwest::Client,
    base_url: Url,
}

pub struct NewEvent<'a> {
    pub source: &'a str,
    pub r#ref: &'a str,
    pub message: &'a str,
    pub description: Option<&'a str>,
    pub severity: Option<Severity>,
    pub occurred_at: Option<jiff::Timestamp>,
    pub active: Option<bool>,
}

#[derive(Copy, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Emergency, Alert, Critical, Error, Warning, Notice, Info, Debug,
}

impl CanopyClient {
    pub fn new(device_key_pem: &str, base_url: Url) -> Result<Self> { ... }
    pub async fn post_event(&self, event: NewEvent<'_>) -> Result<()> { ... }
}
```

Construction:
1. `rcgen::KeyPair::from_pem(device_key_pem)` parses the PKCS8 PEM.
2. `CertificateParams::new(vec!["device.local".into()])?` then
   `params.self_signed(&keypair)?` produces a self-signed cert. Validity
   30 days, regenerated on each `CanopyClient::new` (so daemon SIGHUP /
   reload picks up rotated keys).
3. Concatenate `cert.pem()` + `keypair.serialize_pem()` and pass to
   `reqwest::Identity::from_pem(...)`.
4. `reqwest::Client::builder().identity(identity).build()`.

Event POST: serialize NewEvent as JSON (camelCase `occurredAt`; rest as named),
POST to `{base_url}/events`, expect 200. Log + return error on non-200.

## alertd target wiring

`crates/alertd/src/targets/canopy.rs` (new):

```rust
#[derive(serde::Deserialize, Debug, Clone)]
pub struct TargetCanopy {
    pub canopy: CanopyTargetConfig,
}
#[derive(serde::Deserialize, Debug, Clone)]
pub struct CanopyTargetConfig {
    #[serde(default = "default_canopy_url")]
    pub url: Url,
    pub source: String,
    #[serde(default)]
    pub severity: Option<Severity>,
}
fn default_canopy_url() -> Url { "https://meta.tamanu.app".parse().unwrap() }
```

The `canopy:` nested object is the unique discriminator for serde untagged —
neither email (`addresses`) nor slack (`webhook`) has a `canopy` field, and
adding a third URL-typed top-level field next to those two would be brittle.

`crates/alertd/src/targets.rs`:
- Add `Canopy(TargetCanopy)` to `TargetConnection`.
- `ResolvedTarget` gains a `target_id: String` field so the ref can include it.
- Extend `ResolvedTarget::send` match arms (trigger path).
- Add `ResolvedTarget::send_clear(&self, ctx: &InternalContext, alert: &AlertDefinition) -> Result<()>`:
  - Canopy variant: POST a minimal `active: false` event with the same ref.
  - Other variants: no-op (return Ok).
- **Refactor**: change `send`'s `http_client: Option<&reqwest::Client>`
  parameter to `ctx: &InternalContext` (carries both http_client and the
  optional canopy_client). Touches `alert.rs::execute`, `events.rs::trigger_event`,
  `scheduler.rs::spawn_alert_task`, `http_server/endpoints/{alert,reload}.rs`,
  and the test helpers in `tests/alert_features.rs`.

Render flow in canopy send (trigger):
- `subject` (already rendered, ≤ 200 chars truncated if needed) → `message`
- `body` (already rendered) → `description`
- `severity` from target config, default `Error`
- `source` from target config
- `ref` = `format!("{hostname}/{alert_stem}:{target_id}")` — `target_id` is
  the external-target id, threaded through `ResolvedTarget` (new field
  `target_id: String`).
- `occurred_at` = `now` for v1.
- `active` = `Some(true)`.

Render flow in canopy send (clear):
- `message` = `"alert cleared"` (fixed, no template).
- `description` = `None`.
- `severity` = same as trigger config (canopy mostly uses this to gate
  issue-opening, not closing; setting it consistent avoids surprises).
- `source`, `ref` same as trigger so canopy dedups onto the same issue.
- `occurred_at` = `now`.
- `active` = `Some(false)`.

Dry-run prints `Recipients: canopy:<url>`, subject, body, ref, active —
matching `targets/slack.rs:73-80` pattern.

`crates/alertd/src/lib.rs` + `daemon.rs`:
- `DaemonConfig` gains `device_key_pem: Option<String>` (used at startup only).
- `DaemonConfig::with_device_key_pem` builder method.
- Don't derive Debug on the PEM-bearing field, or wrap in a redacted newtype.
- In `daemon::run_with_shutdown_and_reload`, if `device_key_pem` is set,
  call `CanopyClient::new` once and store on `InternalContext.canopy_client`.
- `InternalContext` gains `canopy_client: Option<Arc<CanopyClient>>`.
- The canopy target send fails loudly if `canopy_client` is None and the user
  has a canopy target configured — `error!("canopy target configured but no device key provided")`.

`crates/alertd/src/main.rs`:
- Add `--device-key-file <PATH>` (env `DEVICE_KEY_FILE`) to `DaemonArgs`.
- `build_daemon_config` reads the file (utf-8 PEM), passes to DaemonConfig.

`crates/alertd/src/targets/default.rs`:
- `determine_default_target` should ignore canopy targets (canopy without a
  configured severity/source on the synthetic event would be confusing).
  Easiest: filter out `TargetConnection::Canopy` before the alphabetical fallback.

`crates/alertd/src/events.rs`:
- The synthetic-alert path (`trigger_event`) needs the same context refactor.

`crates/alertd/src/scheduler.rs`:
- In `spawn_alert_task` clear branch (`if was_triggered { state.triggered_at = None; ... }`),
  iterate `resolved_targets` and call `target.send_clear(&ctx, &alert).await`
  before zeroing state. Errors logged but don't block state transition.
- Also update `metrics::inc_alerts_cleared()` (new counter, optional).

## Legacy `bestool tamanu alerts` target wiring

`crates/bestool/src/actions/tamanu/alerts/canopy.rs` (new): same duplicated
module as alertd's.

`crates/bestool/src/actions/tamanu/alerts/targets/canopy.rs` (new):

```rust
#[derive(serde::Deserialize, Clone, Debug)]
pub struct TargetCanopy {
    #[serde(default = "default_canopy_url")]
    pub url: Url,
    pub source: String,
    #[serde(default)]
    pub severity: Option<Severity>,
}
```

`crates/bestool/src/actions/tamanu/alerts/targets.rs`:
- Add `Canopy { subject: Option<String>, template: String, #[serde(flatten)] conn: TargetCanopy }` variant to `SendTarget`.
- Add matching `Canopy { id, #[serde(flatten)] conn }` to `ExternalTarget`.
- Extend `resolve_external` to map external Canopy → SendTarget::Canopy.
- Extend `SendTarget::send` match (trigger path).
- Add `SendTarget::send_clear(&self, ctx: Arc<InternalContext>, alert: &AlertDefinition) -> Result<()>`:
  - Canopy variant: minimal `active: false` event with same ref.
  - Other variants: no-op.

`crates/bestool/src/actions/tamanu/alerts/templates.rs`:
- Add Canopy to the match in `load_templates` (treat the same as Slack —
  subject + template only, no requester).

`crates/bestool/src/actions/tamanu/alerts/command.rs`:
- Add `canopy_client: Option<Arc<CanopyClient>>` field to `InternalContext`.
- After connecting the postgres client, query `SELECT value FROM local_system_facts WHERE key = 'deviceKey'`.
- If found, build a `CanopyClient` (default url; per-target url overrides happen at send time anyway) and stash on InternalContext.
- If no row (or query fails), leave None and log `info` (legacy command may run on non-Tamanu nodes).
- **Clear path**: after `read_sources` returns Break (no rows / shell
  success), check whether the alert has any canopy targets and call
  `send_clear` for each. Since the legacy command has no state, this fires
  every cron invocation that the alert isn't triggered — canopy is idempotent
  on repeated clears, so this is wasted bandwidth but correct. Update
  `definition.rs::AlertDefinition::execute` to invoke this path before
  short-circuiting.

## `bestool tamanu alertd` wrapper wiring

`crates/bestool/src/actions/tamanu/alertd.rs`:
- In `build_config`, after building `database_url`, open a transient
  postgres connection (or reuse `bestool_postgres::pool`) and fetch
  `deviceKey` from `local_system_facts`. If present, pass through
  `DaemonConfig::with_device_key_pem`.
- No CLI flag here — this wrapper is opinionated about getting it from Tamanu.

## Severity mapping

Send-block can specify `severity: warning` (or any of the canopy enum
strings). If omitted, defaults to `Error`. The eight RFC 5424 severities are
listed in the OpenAPI Severity component: emergency, alert, critical, error,
warning, notice, info, debug.

## ref format

`format!("{hostname}/{alert_stem}:{target_id}")` where:
- `hostname` = `System::host_name().unwrap_or("unknown".into())` (matches
  `templates::build_context`).
- `alert_stem` = `alert.file.file_stem()` to string, or
  `"alert"` if missing.
- `target_id` = the external-target id from `_targets.yml` (the legacy
  command can use `"send"` since its targets are inline).

## Tests

- Unit tests in each `targets/canopy.rs` for YAML parsing of:
  - SendTarget with canopy conn (legacy, tagged).
  - External target canopy entry in `_targets.yml` (alertd, untagged-by-nested-key).
  - Default severity / default url.
- `CanopyClient::new` with the existing fixture PEM from
  `meta_ticket.rs::test_derive_public_key_pem` — assert no error and
  client built. (Skip the network round-trip; that's an integration concern.)
- Update existing `tests/alert_features.rs` constructions of
  `InternalContext` (10+ call sites) to include the new `canopy_client: None`
  field.

## Critical files to touch

- `Cargo.toml` (workspace reqwest features + add rcgen)
- `crates/alertd/Cargo.toml` (add rcgen)
- `crates/alertd/src/lib.rs` (DaemonConfig field + builder)
- `crates/alertd/src/canopy.rs` (new)
- `crates/alertd/src/targets.rs` (Canopy variant, ResolvedTarget refactor)
- `crates/alertd/src/targets/canopy.rs` (new)
- `crates/alertd/src/targets/default.rs` (filter out canopy)
- `crates/alertd/src/daemon.rs` (build canopy_client, populate InternalContext)
- `crates/alertd/src/alert.rs` (InternalContext field, execute → &InternalContext)
- `crates/alertd/src/events.rs` (trigger_event → &InternalContext)
- `crates/alertd/src/scheduler.rs` (spawn_alert_task → &InternalContext)
- `crates/alertd/src/http_server/endpoints/{alert,reload}.rs` (test ctx)
- `crates/alertd/src/main.rs` (--device-key-file flag)
- `crates/alertd/tests/alert_features.rs` (ctx construction updates)
- `crates/bestool/Cargo.toml` (rcgen + p256 in tamanu-alerts feature)
- `crates/bestool/src/actions/tamanu/alerts/canopy.rs` (new)
- `crates/bestool/src/actions/tamanu/alerts/targets.rs` (Canopy variant)
- `crates/bestool/src/actions/tamanu/alerts/targets/canopy.rs` (new)
- `crates/bestool/src/actions/tamanu/alerts/templates.rs` (Canopy match arm)
- `crates/bestool/src/actions/tamanu/alerts/command.rs` (load deviceKey,
  build CanopyClient, put on InternalContext)
- `crates/bestool/src/actions/tamanu/alertd.rs` (load deviceKey, pass to
  DaemonConfig)

## Verification

- `cargo check` for default features + Windows-GNU target (per
  AGENTS.md rule for Windows-specific code paths).
- `cargo clippy --all-targets --all-features` clean.
- `cargo fmt`.
- `DATABASE_URL=postgresql://localhost/tamanu_meta cargo test -p bestool-alertd`
  exercises ctx-construction changes.
- `DATABASE_URL=... cargo test -p bestool` exercises legacy command parsing.
- Manual smoke: write a minimal alert YAML with a canopy target, run
  `bestool-alertd run --dry-run --glob ./alerts --device-key-file ./key.pem`,
  confirm the payload print includes the expected source/ref/severity.
- Manual end-to-end against a staging canopy (if available) is out of
  scope for the merge but the user can do it post-merge.

## Out of scope (deliberately)

- Per-row `ref` (one issue per matching SQL row) — `ref` is single-valued
  per send.
- `occurredAt` derived from row timestamps — always `now`.
- Per-target severity from numerical thresholds (alert vs warn levels).
- Anything related to the canopy server's internal-API endpoints (operator
  paths) — devices use the public `/events` only.
- State-aware clear-send for the legacy command (it sends `active: false`
  every cron invocation that doesn't trigger; canopy dedups so this is
  correct but wasteful). Optimising this requires persisting state across
  cron invocations, which is what `bestool-alertd` is for.
