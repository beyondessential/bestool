# K1 — Substrate abstraction for alertd checks

Design settled in the interview is written up in [SUB](../../specs/tamanu/substrate.md).
This plan holds the technical notes and the outstanding decisions.

## Canopy API: swap to `bes-canopy-api`

`bes-canopy-api` 1.0.0 is on crates.io, built and published by canopy itself.
It replaces the generated half of `bestool-canopy`, so `crates/canopy/build.rs` goes away entirely and with it:

- the live fetch of `https://meta.tamanu.app/api/openapi.json` at build time, and the hard-error-on-failure behaviour that made every build depend on canopy being reachable;
- `openapi.snapshot.json`, the `CANOPY_OPENAPI_OFFLINE` opt-in, and the `DOCS_RS` fallback;
- the `typify` / `schemars` / `prettyplease` / `syn` / `blake3` build-dependencies;
- the snapshot-refresh step in `.github/workflows/release-plz.yml`, which is kept in step with `SPEC_URL` by hand.

Checked against the published crate rather than assumed: it already carries everything `rewrite_types()` was patching in by string substitution — `jiff::Timestamp` throughout with no `chrono`, `Redacted` around `repo_password`, `secret_access_key` and `session_token`, plus `bon` builders and `#[non_exhaustive]` on the generated structs.
So the substitution asserts that guard those rewrites have nothing to replace them with, and simply go.

It also exports `CanopyClient`, `CanopyTransport`, `Redacted` and the error types, which `bestool-canopy` currently defines itself in `client.rs`, `transport.rs` and `error`.
`bestool-canopy` keeps `registration.rs`, `backup.rs` and `reqwest_transport.rs`, with the reqwest transport implementing `bes_canopy_api::CanopyTransport`.
Only 9 sites across the workspace name `bestool_canopy::schema::`, so the re-export surface to get right is small.

`StatusPayload` also arrives as a typed struct with a flattened `extra` map, where the old `is_open_schema` path degraded it to `serde_json::Value`.

## The reporting shape

`POST /status/{server_id}` — the path id is the machine's; the `server_id` name is kept transitionally.

- `StatusPayload { source, machine: Option<TargetReport>, applications: Option<HashMap<String, ApplicationReport>>, health, healthy, extra }`
- `TargetReport { detail, health }` — a machine and an application are described identically.
- `ApplicationReport { type_, detail, health }`, keyed in the map by a reporter-chosen key.

Sending `machine` is what opts a push into the split format; a push without it is treated as the unified legacy one and canopy separates the grains itself.
`source` should be set explicitly to `alertd` rather than relying on the default attribution, since the field becomes mandatory.

`GET /machines/self` returns `{ device_id, machine_id, applications: [type] }`.

## Server identity

`get_or_create_server_id()` in `crates/tamanu/src/server_info.rs` mints and persists an id to a host file, and is called from `doctor/sweep.rs:350` and `bestool/src/actions/tamanu/doctor.rs:228`.
Under the split it is the *machine's* identity, which is the correct thing for it to be: minted once by the agent on the box it is enrolled for.
It needs renaming to match, and both call sites need to stop treating it as an application's identity.

## The application key

The key only has to separate applications within one machine — canopy correlates on machine plus key, and mints its own internal application id — so there is nothing for it to encode and no need to derive it from anything.

bestool on Windows and Linux uses a static key for the Tamanu application it reports.
Two Tamanu applications on one host is not a shape that is run today, and if it lands it needs deliberate adaptation across the board, of which bestool is one part; a derived key would not save that work.

The substrate API takes the key from its caller instead, so a process driving many applications supplies each one's. For Kubernetes that is expected to be built from the namespace, the role, and an id, but that belongs to the relay rather than here.

## Open

- **Whether `bestool-canopy` re-exports `bes-canopy-api`'s client or wraps it.** Re-export is less code; a wrapper keeps the option of adding bestool-side behaviour without touching call sites.

## Build steps

- [ ] Swap `bestool-canopy` onto `bes-canopy-api`, delete `crates/canopy/build.rs` and the snapshot, drop the release-plz refresh step
- [ ] Introduce the substrate trait and the check-storage trait, with own-system implementations
- [ ] Split the check registry into machine and application subjects
- [ ] Port the duty vocabulary, replacing supervisor unit-name matching in `tamanu_service` and `version_drift`
- [ ] Add per-service resource metrics, graded only against a declared ceiling
- [ ] Split the reported facts into the machine and application `detail` blocks
- [ ] Rename the server id to the machine id and fix both call sites
