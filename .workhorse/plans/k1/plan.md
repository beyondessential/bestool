# K1 — Substrate abstraction for alertd checks

Design settled in the interview is written up in [SUB](../../specs/tamanu/substrate.md).
This plan holds the technical notes and the outstanding decisions.

## Blocked on Y1

`Y1` swaps `bestool-canopy` onto the `bes-canopy-api` crate and re-exports its client, transport, error and schema types.
That work is not in this card's scope; what this card needs from it is the typed `StatusPayload` below.

Until Y1 lands, `StatusPayload` is degraded to `serde_json::Value` by the old `is_open_schema` path, so the machine and application sections would have to be hand-built as raw JSON and then deleted.
That is the reason this card waits rather than working around it.

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

## Build steps

- [ ] Introduce the substrate trait and the check-storage trait, with own-system implementations
- [ ] Split the check registry into machine and application subjects
- [ ] Port the duty vocabulary, replacing supervisor unit-name matching in `tamanu_service` and `version_drift`
- [ ] Add per-service resource metrics, graded only against a declared ceiling
- [ ] Split the reported facts into the machine and application `detail` blocks
- [ ] Rename the server id to the machine id and fix both call sites
