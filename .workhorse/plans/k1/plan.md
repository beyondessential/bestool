# K1 — Substrate abstraction for alertd checks

Design settled in the interview is written up in [SUB](../../specs/tamanu/substrate.md).
This plan holds the technical notes and the outstanding decisions.

## Ground already laid by Y1

`bestool-canopy` now re-exports `bes_canopy_api`'s `schema`, transport, error and `Redacted` types, with a `CanopyClient<T = ReqwestTransport>` alias restoring the default transport parameter.
The constructors are free functions in `connect.rs` (`connect`, `connect_to`), and `is_tailscale`, `refresh` and `renew` live on `ReqwestTransport`.

So the typed `StatusPayload` this card reports through is already reachable as `bestool_canopy::schema::StatusPayload`, and `doctor/task.rs:306` already deserialises the sweep's JSON into it.

## The reporting shape

`POST /status/{server_id}` — the path id is the machine's; the `server_id` name is kept transitionally.

- `StatusPayload { source, machine: Option<TargetReport>, applications: Option<HashMap<String, ApplicationReport>>, health, healthy, extra }`
- `TargetReport { detail, health }` — a machine and an application are described identically.
- `ApplicationReport { type_, detail, health }`, keyed in the map by a reporter-chosen key.

Sending `machine` is what opts a push into the split format; a push without it is treated as the unified legacy one and canopy separates the grains itself.
`source` should be set explicitly to `alertd` rather than relying on the default attribution, since the field becomes mandatory.

`GET /machines/self` returns `{ device_id, machine_id, applications: [type] }`.

`build_payload` at `doctor/sweep.rs:515` is where the flat payload is assembled today, and is the seam where the machine and application sections get split apart.

## Server identity

`get_or_create_server_id()` in `crates/tamanu/src/server_info.rs` mints and persists an id to a host file, and is called from `doctor/sweep.rs:350` and `bestool/src/actions/tamanu/doctor.rs:228`.
Under the split it is the *machine's* identity, which is the correct thing for it to be: minted once by the agent on the box it is enrolled for.
It needs renaming to match, and both call sites need to stop treating it as an application's identity.

## The application key

The key only has to separate applications within one machine — canopy correlates on machine plus key, and mints its own internal application id — so there is nothing for it to encode and no need to derive it from anything.

bestool on Windows and Linux uses a static key for the Tamanu application it reports.
Two Tamanu applications on one host is not a shape that is run today, and if it lands it needs deliberate adaptation across the board, of which bestool is one part; a derived key would not save that work.

The substrate API takes the key from its caller instead, so a process driving many applications supplies each one's. For Kubernetes that is expected to be built from the namespace, the role, and an id, but that belongs to the relay rather than here.

## Split out to A2

`A2` files every check and fact against its subject and pushes the split format, which needs no substrate: the subject is a static property of each check, and on a host bestool already knows it is the machine and knows its one application.
This card builds on that, adding the part that makes the split work when the checking process is *not* on the machine.

Gone from here with it: the registry subject split, the facts split, and the machine id rename.
`SUBJ` (`.workhorse/specs/tamanu/subjects.md`) is A2's spec; `SUB` is this card's.

## Open: is a uniform substrate the right shape?

Working through A2 suggests it may not be. A2 does the whole subject split with no substrate, and does not feel like it is working around a missing abstraction.

Three of the four things `SUB` has a substrate answering for are not substrates: a database connection, Tamanu's config and version, and "am I the machine" are parameters and a boolean. Only the workload grouping — duties, services, per-service facts — abstracts genuinely different acquisition. The other three restate the old `@tamanu` / `@db` / `host` categories.

The suite is the bigger issue. 18 of 45 checks are machine checks and a relay assembles none of them, so 40% of the catalogue would exist only to skip, every sweep, burying the skips that mean something.

The alternative: expose the checks and the shared machinery, and let a consumer assemble the suite it needs, rather than running alertd as one API with a substrate plugged into it. Under that model "not applicable to this environment" is absence from the suite and "applicable but unreadable today" stays a skip — two facts the current spec collapses into one. Canopy supports both: a source's push only opens and recovers its own checks.

What this would preserve, drop, and raise:

- **Preserved, and more explicitly**: the property that the two environments cannot diverge into subtly different checks, because the shared unit becomes the check itself rather than the whole sweep.
- **Still needed**: the abstraction over how a reading that feeds a threshold is obtained. Assembly owns *which* checks run; if a consumer supplies the numbers instead of the check asking for them, two consumers can grade against different denominators. The seam narrows rather than disappears.
- **Dropped**: the runtime "machine checks skip when not on the machine" guard, mostly — a relay that never assembles machine checks does not need it, though it stays cheap insurance against assembling one by mistake.
- **Kept regardless**: the duty vocabulary, per-service metrics, and check storage as an injectable.
- **To avoid**: hand-assembly by check name, which drifts as the catalogue grows. Better that the registry survives, each check declares what it requires, and a consumer filters it, so a new check propagates by default and exclusion is deliberate.
- **Raised**: whether `bestool-alertd` splits into a checks-and-machinery library and a daemon that is one consumer of it. Larger than anything currently on this card.

## Build steps

- [ ] Introduce the substrate trait and the check-storage trait, with own-system implementations
- [ ] Make machine checks skip when the substrate is not the machine
- [ ] Port the duty vocabulary, replacing supervisor unit-name matching in `tamanu_service` and `version_drift`
- [ ] Add per-service resource metrics, graded only against a declared ceiling
- [ ] Take `pg_tuning`'s denominator from the Postgres service's declared ceiling, removing A2's interim machine-memory read
- [ ] Scope check storage per subject, retiring the fixed cache path `http_errors` and `external_users` share
