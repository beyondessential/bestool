# Swap bestool-canopy onto the bes-canopy-api crate

Replace the build-time-generated half of `bestool-canopy` with the published
`bes-canopy-api` 1.0.0 crate, re-exporting its client/transport/error/schema
types rather than wrapping them.

## Key design decisions

- **`CanopyClient` becomes a type alias with a default transport**:
  `pub type CanopyClient<T = ReqwestTransport> = bes_canopy_api::CanopyClient<T>;`
  so every `Arc<CanopyClient>` mention across the workspace keeps working. The
  published client has no default; the alias restores it.
- **Constructors move to free functions.** The old `CanopyClient::new` /
  `CanopyClient::with_urls` built a `ReqwestTransport` and wrapped it. Rust
  can't hang inherent methods off the foreign alias, so they become
  `bestool_canopy::connect` / `bestool_canopy::connect_to`, returning
  `Result<Option<CanopyClient>>` (miette — transport construction is this
  crate's concern).
- **Transport-shaped methods live on `ReqwestTransport`.** `is_tailscale`,
  `refresh`, `renew`, and the `raw-requests`-gated `get`/`request` are already
  inherent methods there. The `CanopyClient` wrappers go; call sites reach them
  through `.transport()`.
- **Error boundary pushes out to call sites.** Generated methods now return
  `bes_canopy_api::Result` (`thiserror`-based `Error`), not `miette::Result`.
  `Error: std::error::Error`, so call sites add `.into_diagnostic()`. Downcasts
  to `CanopyHttpError` become `err.http()`.

## Checklist

### Core crate: bestool-canopy

- [x] `Cargo.toml`: add `bes-canopy-api = "1.0.0"` (done); drop the whole
      `[build-dependencies]` table and the `blake3` dependency; drop `bon` if
      unused after the rewrite (schema builders come from the api crate now).
- [x] Delete `build.rs` and `openapi.snapshot.json`.
- [x] `lib.rs`: re-export `schema`, `CanopyClient` (alias), `CanopyTransport`,
      `CanopyRequest`, `CanopyResponse`, `Redacted`, `CanopyHttpError`, `Error`,
      `async_trait` from `bes_canopy_api`. Drop the local `Redacted` definition
      and the local `schema`/`transport` modules. Keep `backup`, `registration`,
      `reqwest_transport`, `test_support`. Add the `connect`/`connect_to` free
      functions (own module or here).
- [x] Delete `client.rs` (client + send machinery now in the api crate) and
      `transport.rs` (re-exported). Relocate the still-valuable transport-level
      tests (default-transport build, user-agent on the wire) into
      `reqwest_transport.rs` or the connect module.
- [x] `reqwest_transport.rs`: `impl bes_canopy_api::CanopyTransport` with `call`
      returning `bes_canopy_api::Result`; map reqwest/url errors to
      `Error::transport`. Keep `is_tailscale`/`refresh`/`renew`/`get`/`request`
      inherent and miette-based.
- [x] `backup.rs`: use re-exported `CanopyHttpError`; rewrite
      `TargetOutcome::from_result` to take `bes_canopy_api::Result<BackupTarget>`
      and branch on `err.status()` (412/409 → Dormant).
- [x] `tests/schema.rs`: unchanged (schema re-exported) — confirm it still builds.

### External call sites (12 files)

- [x] Constructors: `CanopyClient::new(...)` → `connect(...)`,
      `CanopyClient::with_urls(...)` → `connect_to(...)` in daemon.rs, tags.rs,
      psql.rs, tamanu/doctor.rs, canopy/backup.rs.
- [x] `.is_tailscale()` / `.renew()` on a client → `.transport().is_tailscale()`
      / `.transport().renew()` (daemon.rs x3, tags.rs, psql.rs).
- [x] Generated-method awaits: add `.into_diagnostic()` where the miette `?` /
      `wrap_err` boundary now sees `bes_canopy_api::Error`.
- [x] `err.downcast_ref::<CanopyHttpError>()` → `err.http()`
      (canopy_registration.rs).
- [x] `TargetOutcome::from_result(...)` callers: pass the api result through
      (kopia.rs, restore.rs, backup.rs) and `.into_diagnostic()?` the outcome.

### CI

- [x] `.github/workflows/release-plz.yml`: remove the "Refresh canopy OpenAPI
      snapshot" step.

### Verify

- [x] `cargo build`, `cargo clippy`, `cargo fmt` across the workspace; tests
      pass with no build-time network access.

## Notes as built

- **`status()` payload.** The generated `status` now takes `&StatusPayload`, not
  `&serde_json::Value`. The doctor sweep still builds its payload as a free-form
  JSON object; the conversion happens at the call boundary in `task.rs` via
  `serde_json::from_value::<StatusPayload>`. Sound because the health vocabulary
  the sweep emits (`passed`/`warning`/`failed`/`broken`/`skipped`) matches
  `CheckResult`, and everything else lands in the flattened `extra` maps of
  `StatusPayload`/`HealthCheck`. K1 will populate `machine`/`applications`
  directly; Y1 only makes the typed struct available on the wire.
- **Typed responses.** `bestool_snippets` and `status_check_severities` now
  return typed maps, so the `serde_json::from_value` decode steps at their call
  sites (`psql.rs`, `tamanu/doctor.rs`) are gone — the values are used directly
  (snippets mapped `SnippetResponse` → local `Snippet`).
- **release-plz.toml.** Removed the whole `bestool-canopy` package block: its
  only setting, `publish_allow_dirty`, existed for the uncommitted snapshot,
  which is gone.
- **Deps dropped from the canopy crate:** the `[build-dependencies]` table
  (`typify`, `schemars`, `prettyplease`, `syn`, `blake3`, blocking `reqwest`,
  `serde`/`serde_json`) plus runtime `bon`, `flate2`, `bytes` (all now the api
  crate's concern). `blake3` stays as a runtime dep — `registration.rs` uses it.
- **Test caveat:** two alertd `http_server` endpoint tests fail in the sandbox
  with `postgres: connection closed` at pool creation — a DB-infra issue in this
  environment, not touched by this change. Everything else passes.
