# Expected-services definition for Tamanu

## Problem

`tamanu doctor`'s `tamanu_service` check currently enumerates loaded `tamanu-*`
services and flags any that aren't running. This:

- (a) doesn't notice services that *should* be up but aren't even loaded;
- (b) sweeps in unrelated services that happen to be named `tamanu-*`;
- (c) can't say "this service is up but shouldn't be".

We want a declarative description of which services are expected to be up
(or explicitly down) given:

- the supervisor (systemd or pm2);
- the server kind (Central or Facility);
- bits of the Tamanu config (currently `fhir.worker.enabled`).

That description will be reused later by other commands (e.g. the WIP rolling
reload in #313), so it lives in its own module rather than inside the doctor
check.

## Initial ruleset

The first cut encodes only what's needed right now, with shape designed to
grow as the matrix expands.

- `tasks`: exactly one must be up (both kinds, both supervisors).
- Systemd-only:
  - `frontend`: exactly two up, instances `@a` and `@b`. Not kind-prefixed
    (the unit is `tamanu-frontend@<x>.service`).
  - `tamanu-facility` (a singleton legacy unit, not kind-prefixed): must NOT
    be active or enabled.
- Central:
  - `api`: at least two up.
  - If `config.fhir.worker.enabled` is true:
    - `fhir-resolve`: exactly one up.
    - `fhir-refresh`: exactly one up.
- Facility:
  - `api`: at least two up.
  - `sync`: exactly one up.

## Naming

- pm2: `tamanu-${thing}`. Multiple instances of one logical service share
  the same name (pm2 doesn't use `@instance` notation).
- systemd: `tamanu-${kind}-${thing}` by default; instances `@1`, `@2`, …
  - `frontend` is the exception: `tamanu-frontend` with instances `@a`, `@b`
    (no kind prefix).
  - `tamanu-facility` is the explicit "must not be present" singleton (no
    kind prefix, no `${thing}` segment beyond the literal).

## Data model

New module: `crates/bestool/src/actions/tamanu/services.rs`.

```rust
pub enum Supervisor { Systemd, Pm2 }
pub enum ServerKind { Central, Facility }

pub struct Expectation {
    /// Concrete service base name as the supervisor sees it
    /// (e.g. "tamanu-facility-api" on systemd, "tamanu-api" on pm2,
    /// "tamanu-frontend", or "tamanu-facility").
    pub name: String,
    pub instances: Instances,
    pub state: ExpectedState,
}

pub enum Instances {
    /// Singleton — `name` itself (no `@`).
    Single,
    /// At least N instances, named `@1`, `@2`, …
    NumericAtLeast(usize),
    /// Exactly these instance suffixes, e.g. `["a", "b"]`.
    Named(&'static [&'static str]),
}

pub enum ExpectedState { Up, Down }

pub fn expected(
    supervisor: Supervisor,
    kind: ServerKind,
    config: &TamanuConfig,
) -> Vec<Expectation>;
```

Notes:

- `Instances::Single` is "must have exactly one matching unit/process and no
  `@instance` suffix". For pm2 this is "one process by that name". For
  systemd it's `name.service` (no template instance).
- `Instances::Named` is currently systemd-only (only `frontend` uses it).
  On pm2 it'd degrade to "must have len() processes by that name" if ever
  needed, but we don't generate one for pm2 today.
- `Instances::NumericAtLeast(n)` covers "at least 2 api". On systemd we count
  units named `${name}@${digits}.service`. On pm2 we count processes named
  `${name}` (pm2 doesn't carry instance suffixes).
- `ExpectedState::Down` services don't need to be "found"; matching is "is
  any matching unit/process present?". For systemd we also surface
  `is-enabled`, since "enabled but inactive" is still forbidden.

## Config extension

Add to `TamanuConfig`:

```rust
pub fhir: Option<FhirConfig>,
```

with

```rust
#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct FhirConfig {
    pub worker: FhirWorkerConfig,
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct FhirWorkerConfig {
    pub enabled: bool,
}
```

A helper `TamanuConfig::fhir_worker_enabled()` returns `false` when the
section is missing.

## Doctor integration

Rewrite `crates/bestool/src/actions/tamanu/doctor/checks/tamanu_service.rs`:

1. Detect supervisor from platform (linux → systemd, windows → pm2).
2. Compute `ServerKind` from `ctx.config.is_facility()`.
3. Call `services::expected(supervisor, kind, &ctx.config)`.
4. Enumerate live units/processes:
   - systemd: `systemctl list-units --type=service --all tamanu-*.service`
     plus `systemctl is-enabled` for any `Down` expectations (so we catch
     enabled-but-inactive).
   - pm2: `pm2 jlist`.
5. For each expectation:
   - `Up`: count matches against the instance pattern. Diagnose
     `missing`, `instance shortfall`, or `unexpected absent`.
   - `Down`: any matching unit that is active OR enabled is a failure.
6. Any *extra* `tamanu-*` services not matched by any expectation become
   warnings (point (b) of the problem statement) — not failures, since
   ops may legitimately run extras during maintenance.
7. The check `Fail`s if any required `Up` is missing or any `Down` is
   present; otherwise `Pass`. Extras flip it to `Warning` if no fatal
   issue.

The check exposes structured details: `expected` (with state + instance
spec), `services` (discovered), `missing`, `forbidden`, `extras`.

## Out of scope (for now)

- Wiring this into the reload subcommand (#313) — that lives behind its own
  PR. The model is shaped to make adoption straightforward (a "what should
  be restarted" list is `expected().filter(state == Up)`).
- Custom thresholds (e.g. warn vs fail on api-instance shortfall). Today
  any shortfall is a fail.
- Reading per-instance pm2 configs to validate intent vs runtime; pm2 is a
  flat process list and that's all we use.

## Implementation order

1. Data model + `expected()` (with unit tests over the matrix).
2. Config extension for `fhir.worker.enabled`.
3. Doctor check rewrite, with matching tests fed synthetic discovered lists.
4. Lint + format.
