# K1 — Substrate abstraction for alertd checks

Design written up in [SUB](../../specs/tamanu/substrate.md); the subjects checks report for are in [SUBJ](../../specs/tamanu/subjects.md).
This plan holds the technical notes and the outstanding decisions.

## Ground already laid

Four cards cleared the way, and between them they built more of this than the original brief anticipated.

`Y1` put the canopy API in place: `bestool-canopy` re-exports `bes_canopy_api`'s `schema`, transport, error and `Redacted` types, with a `CanopyClient<T = ReqwestTransport>` alias restoring the default transport parameter.

`A2` shipped the subject split: `Subject::{Machine, Application(ApplicationRef)}`, applicability deciding that a check not admitted by a subject **never runs and carries no result**, check names qualified `subject:name`, Postgres as its own application keyed by port, and `get_or_create_machine_id`.

`K2` moved the daemon into the `bestool` binary. The checks crate is now flat — `checks.rs`, `check.rs`, `heal.rs`, `stat.rs`, `subject.rs`, `sweep.rs` at the root, with no `doctor/` wrapper.

`L2` built the per-subject context and split the signature:

- `MachineCx { http, canopy, tamanu: Option<MachineTamanu> }` and `AppCx { app, version, config, install_root, database_url, pool, http }`.
- `Run::{Machine(Runner<MachineCx>), Application(AppScope, Runner<AppCx>)}`, with the heal inside the arm so a cross-arm heal is a type error where it is written.
- `AppScope::{Postgres, Tamanu, Central, Facility}` — `CheckScope`'s `Machine` variant is gone.
- Heal attempts are tracked per instance, so two applications running one failing check each get their own allowance.
- Each application carries its own `PgPool`, which answers `M1` inside `L2` rather than leaving it here.
- A cluster's context is built from the cluster rather than from the Tamanu, so a cluster no longer answers with another application's configuration or install root.

What is left for this card is the substrate itself: the runtime a check reads an application through, the storage it remembers readings in, the duty vocabulary, and per-service resource usage.

## Settled: `AppCx` splits, and the runtime splits with it

`L2` left this deliberately, as a card-shape question rather than a review fix. Its plan recorded that review raised the same smell twice — once for `install_root` and `config`, once for `kind` — and that each round fixed a field rather than the shape. Taking the structural answer.

`AppCx` serves both a Tamanu deployment and a Postgres cluster, and three of its fields are degenerate for a cluster: `version` is the `0.0.0` unresolved marker, `install_root` is always `None`, and `config` is a synthesised stub naming only the cluster's own database. `server_kind()` returning `Option` is the same shape again.

The structural answer `L2` proposes is a third arm:

```rust
pub enum Run {
    Machine(Runner<MachineCx>),
    Postgres(Runner<PgCx>),
    Tamanu(TamanuScope, Runner<TamanuCx>),
}
```

The scope already determines which, so nothing is lost by encoding it.

### It reaches the substrate too

This is not only `L2`'s leftover: deciding it one way or the other changes what this card builds.

`SUB` has a substrate covering three things — the services running an application, the traffic reaching it, and the certificates in front of it. A Postgres cluster uses the first and neither of the others. It serves no HTTP, and the certificates a check grades are the ones in front of a web front end.

So a single `Runtime` trait handed to both would leave a Postgres runtime answering `Unavailable` for two of its five methods, permanently and by construction — the same degenerate-field smell one level down, in a trait rather than a struct.

If the contexts split, the trait can split with them along the line that already exists in the readings:

- services, per-service facts and compute state are wanted by every application;
- traffic counters and certificates are wanted by an application that serves HTTP.

`PgCx` then holds the first; `TamanuCx` holds both. The cut is by capability rather than by product, so an mSupply application later takes the HTTP half without anything being reshaped for it.

This does resemble the capability-traits option considered early in the interview and rejected in favour of one trait. What has changed is that there is now a concrete second context type to hang the split off, rather than a hypothetical one.

### Two judgement calls inside that

**The arms are named for the products they carry today**, not for the capability that distinguishes them. An mSupply application would later either add a fourth arm or rename the Tamanu one; both are mechanical changes to internal types, and naming a capability abstraction from a single instance is the more expensive mistake.

Worth knowing that two cuts coincide here and are not the same cut. `version`, `config` and `install_root` are *installed product* concerns; traffic and certificates are *serves HTTP* concerns. Everything foreseeable falls the same side of both — a managed Postgres is neither, an mSupply is both — so nothing is built to tell them apart until something needs it.

**The two runtimes are two fields, not one supertrait.** A `WebRuntime: ServiceRuntime` would read neatly, but the readings come from genuinely different sources: services from a supervisor and traffic from the front end on a machine, the cluster API and the gateway on Kubernetes. One object implementing both would make every implementer compose two unrelated things for no benefit at the call site.

## The runtime traits

Narrow, per `SUB`: what genuinely differs between environments. A database connection, a config and a version are parameters on the context, not readings.

```rust
/// What is running an application. Every application has one.
#[async_trait]
pub trait ServiceRuntime: Send + Sync {
    fn compute(&self) -> Compute;                                    // Running | SwitchedOff
    async fn services(&self) -> Result<Vec<Service>, Unavailable>;
    async fn service_facts(&self, id: &ServiceId) -> Result<ServiceFacts, Unavailable>;
}

/// What reaches an application that serves HTTP, and what fronts it.
#[async_trait]
pub trait HttpRuntime: Send + Sync {
    async fn http_counters(&self) -> Result<TrafficCounters, Unavailable>;
    async fn certificates(&self) -> Result<Vec<Certificate>, Unavailable>;
}
```

`PgCx` carries a `ServiceRuntime`; `TamanuCx` carries one of each.

`Unavailable` carries a free-form reason string, which the check turns into its skip. A closed set of causes — not permitted, not reachable, not present — would let canopy grade a permissions problem differently from an outage, and may be worth having later; there is not enough usage yet to know which causes are real, so the string comes first and the set is derived from what actually gets written.

## Check storage: the lifetime is declared at the write

```rust
#[async_trait]
pub trait CheckStore: Send + Sync {
    async fn get(&self, key: &str) -> Option<Vec<u8>>;
    async fn put(&self, key: &str, value: &[u8], lifetime: Lifetime);
    async fn clear(&self, key: &str);
}

pub enum Lifetime {
    /// Read from something that restarts when the application's compute does.
    UntilCompute,
    /// Measures something the application's own data holds.
    Durable,
}
```

The store handed to a check is already scoped to its subject, so a check cannot name another subject's state, and the key namespace is its own.

Three places could carry the discard-on-sleep declaration, and the write wins:

- **A field on `CheckEntry`** puts it in the registry beside the scope, where it is greppable, and lets the machinery clear uniformly. But it sits away from the code that stores anything, so a check that starts storing something and does not update its entry has the bug back. It is also a field that means nothing for the forty-odd checks that store nothing.
- **A field on `Runner`** is the same idea declared once per arm instead of once, which is strictly worse.
- **The check clearing its own store** on seeing `Compute::SwitchedOff` is a few lines in one check today. The count only grows, and the failure mode is the problem rather than the volume: omit it and a stale baseline produces a plausible delta on waking, with no error and no signal.

Declaring at the write cannot be omitted, because there is no way to store without choosing, and the tag travels with the data so it cannot drift from what wrote it. It costs a parameter at each write site, which is one site in most checks that have any.

Of the three checks that hold state today, `external_users` and `ips` report for the machine, so no application's compute ever switches off under them. Only `http_errors` is in scope, and per-service metrics may add more.

### One runtime per application, not one per machine

Each application gets its own runtime, resolved when the sweep builds its list of applications. Detection may share work across them — probing for systemd once — but the result is per-application.

This is required rather than tidier. A Windows machine runs Tamanu under PM2 and Postgres as a native Windows service, so the two applications on one box genuinely have different runtimes, and a single machine-wide supervisor cannot describe both.

A machine subject has no runtime at all: `MachineCx` carries none, because machine checks read the host directly.

## Neighbouring cards

`E2` (discover every Postgres cluster) was waiting on the per-subject context, which `L2` has now landed. Its remaining work is discovery.

`J1` (detect and restart PM2 on Windows) overlaps the Windows runtime: "is PM2 running as a service" is exactly what a PM2 runtime has to answer before it can list any services.

`N1` (per-check timing) touches the dispatch loop this card extends.

`X1` (decide bestool-canopy's role post `bes-canopy-api`) was answered by `Y1` and looks stale.

## Build steps

- [x] Split `AppCx` into `PgCx` and `TamanuCx`, adding the third `Run` arm
- [ ] Introduce the two runtime traits and the check-storage trait, with own-system implementations resolved per application
- [ ] Port the duty vocabulary, replacing supervisor unit-name matching in `tamanu_service` and `version_drift`
- [ ] Add per-service resource metrics, graded only against a declared ceiling
- [ ] Take the Postgres tuning check's denominator from the running service's declared ceiling, falling back to the hosting machine's memory
- [ ] Scope check storage per subject, retiring the fixed cache path `http_errors` and `external_users` share
