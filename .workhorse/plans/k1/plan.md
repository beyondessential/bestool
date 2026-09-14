# K1 — Substrate abstraction for alertd checks

Design written up in [SUB](../../specs/tamanu/substrate.md); the subjects checks report for are in [SUBJ](../../specs/tamanu/subjects.md).
This plan holds the technical notes and the outstanding decisions.

## Ground already laid

`Y1` put the canopy API in place: `bestool-canopy` re-exports `bes_canopy_api`'s `schema`, transport, error and `Redacted` types, with a `CanopyClient<T = ReqwestTransport>` alias restoring the default transport parameter. The constructors are free functions in `connect.rs`, and `is_tailscale`, `refresh` and `renew` live on `ReqwestTransport`.

`A2` then shipped the subject split, and went further than this plan anticipated:

- `Subject::{Machine, Application(ApplicationRef)}` and `CheckScope` in `doctor/subject.rs`, with every registry entry carrying a scope.
- `CheckScope::admits` decides applicability, and a check whose scope does not admit a subject **never runs and carries no result** — absent rather than skipped.
- Check names are qualified `subject:name`, so one name may belong to a machine check and to an application check without the two being the same check.
- A Postgres installation is its own application, keyed by the port it answers on rather than by its version.
- The machine identity is named for what it is: `get_or_create_machine_id`.

Two consequences here. The seam for deciding *which* checks run against *what* already exists as `CheckScope`, so nothing on this card needs to build it. And the guard this plan used to carry — machine checks skipping when the substrate is not the machine — has dissolved: a process with no machine subject never runs them, so there is no wrong-host mode left to defend against.

## Settled: the substrate is narrow, and the suite is assembled

A uniform substrate everything routes through is the wrong shape, and `SUB` has been rewritten accordingly.

Three of the four things it used to have a substrate answering for were not substrates: a database connection, Tamanu's config and version, and "am I the machine" are parameters and a boolean. Only the workload abstracts genuinely different acquisition — services found through a supervisor, a container runtime, or a cluster API — along with the traffic reaching an application and the certificates in front of it. Those three are what a substrate now covers; everything else is supplied to a check rather than asked for.

The suite is assembled rather than fixed, which `CHK` now states. A2 built most of this already: `CheckScope` is the registry-declares-its-own-requirements model filtered per subject, so a new check reaches every consumer whose subjects admit it and no consumer keeps its own list of names. Its applicability-versus-skipped rule is the distinction that model needs — absence means "does not belong to this subject", skip means "belongs here but could not be read on this sweep".

What survives the narrowing is the reason the abstraction exists at all: a reading that feeds a threshold is asked for rather than handed in, so two consumers running one check cannot grade against different denominators.

## Crate shape: move the daemon up, leave the checks

`CHK` requires the checks and machinery to be available independently of the daemon that schedules them. Rather than extract a new checks crate out of `bestool-alertd`, the daemon moves up into the `bestool` binary and the checks stay where they are.

The measurements favour this decisively: roughly 16,700 lines of checks and machinery would stay, against roughly 4,200 that move — `daemon`, `http_server`, `tasks`, `backup`, `child_confinement`, `windows_service`, `context`, `metrics`, `commands`, and `doctor/task.rs`. Extracting the checks would have been the large edit; moving the daemon is the small one. `bestool` is the only crate that depends on `bestool-alertd`, so nothing outside the workspace breaks.

The seam is already clean. `doctor/` reaches into the rest of the crate from exactly one file — `doctor/task.rs`, for `BackgroundTask`, `TaskContext`, `TaskEndpoint` and `TaskEndpointHandler` — and that file is the scheduled-task wrapper, which is daemon-side by nature. The daemon reaches into `doctor/` only for `Stat`, `StatKind`, `MetricsSnapshot`, `StatusCounts` and `DoctorMetricsHandle`, all of it to render `/metrics`, which is the right direction for a consumer to depend on a library.

Putting the daemon in the binary also puts composition where composition belongs: the daemon is the thing that wires a schedule, a server and a canopy client together, and it has exactly one consumer.

The crate keeps the name `bestool-alertd` even though the daemon leaves it. These are published crates with version history behind them, and renaming one costs a new registry entry, a release-plz change and broken documentation links, to buy nothing but tidiness. The name records what the crate was built for, which is still what its checks are for.

Moving the daemon out removes `run`, `DaemonConfig`, `BackgroundTask` and the rest from the crate's public API, so the move is a major version bump regardless. Anything else worth changing about that surface — flattening `doctor::checks::all()` down to `checks::all()`, for one — rides along at no extra cost and should be done in the same bump rather than in a later one.

### Cleanup this enables

`CheckContext` still carries `has_install` and `is_tamanu`. `CheckScope` made the second redundant — a non-Tamanu database is a Postgres application, which `CheckScope::Tamanu` does not admit — so it can go. `has_install` remains a real distinction (an application known only through its database has no install files to read) but is a property of the application rather than a gate each check consults.

## Two check signatures, and a per-subject context

A check is dispatched with a context built for the one subject it reports for, rather than the sweep-wide `SweepContext` every check receives today. Machine checks and application checks take different context types, so a machine check cannot reach an application's runtime or its scoped storage — the compiler enforcing what `CheckScope` enforces at runtime now.

```rust
pub struct MachineCx { store, http, canopy }
pub struct AppCx { app: ApplicationRef, runtime: Arc<dyn Runtime>, store, db, config, http }

pub struct Runner<Cx> {
    run: fn(Cx) -> BoxFuture<'static, Check>,
    heal: Option<HealAction<Cx>>,
}

pub enum Run {
    Machine(Runner<MachineCx>),
    Application(AppScope, Runner<AppCx>),
}
```

Heal travels inside the arm because both arms have one — `canopy_registration` is a machine check and `fhir_jobs` an application check — and a heal needs the same context its check ran with.

The scope moves inside the application arm, so `CheckScope` loses its `Machine` variant and becomes `AppScope` (Postgres, Tamanu, Central, Facility). Today a check carries a scope and a runner as two independent fields, which makes a machine runner paired with `CheckScope::Postgres` representable and wrong; folding the scope into the arm makes that combination not exist.

This also retires the registry's other axis. `entry!("connect", db_connect, db, postgres)` carries both a category (`db`) and a scope (`postgres`), and the category arm still gates on `is_tamanu` — the macro's own comment concedes the skip is by now nearly unreachable. With the context built per subject and its parameters already resolved, there is nothing for the category to decide.

The cost is narrower than it first looks. Only dispatch and heal match on the arm; `name`, `on_wire`, selection by qualified name, progress announcement and declared stats all read `CheckEntry` fields or the resulting `Check`, and none of them change.

### What the per-subject context fixes

Dispatch currently hands every check the same context (`sweep.rs:713`), so a check filed against two subjects of one kind runs twice against whichever one that shared context holds. Only one Postgres application is ever discovered today, so it is not reachable — but `subjects_for` already returns one subject per cluster, and the dispatch is what would have to change for that to mean anything.

`heal::spawn_if_due` keys its rate limit and its one-attempt-in-flight guard on the bare check name, which has the same shape of problem: two applications' heals for one check would share a single limit. The key becomes the qualified name.

## The substrate trait

Narrow, per `SUB`: the services running an application, the traffic reaching it, and the certificates in front of it. Nothing else — a database connection, a config and a version are parameters on `AppCx`, not readings.

```rust
#[async_trait]
pub trait Runtime: Send + Sync {
    fn compute(&self) -> Compute;                                    // Running | SwitchedOff
    async fn services(&self) -> Result<Vec<Service>, Unavailable>;
    async fn service_facts(&self, id: &ServiceId) -> Result<ServiceFacts, Unavailable>;
    async fn http_counters(&self) -> Result<TrafficCounters, Unavailable>;
    async fn certificates(&self) -> Result<Vec<Certificate>, Unavailable>;
}
```

`Unavailable` carries a free-form reason string, which the check turns into its skip. A closed set of causes — not permitted, not reachable, not present — would let canopy grade a permissions problem differently from an outage, and may be worth having later; there is not enough usage yet to know which causes are real, so the string comes first and the set is derived from what actually gets written.

### One runtime per application, not one per machine

Each application gets its own runtime, resolved when the sweep builds its list of applications. Detection may share work across them — probing for systemd once — but the result is per-application.

This is required rather than tidier. A Windows machine runs Tamanu under PM2 and Postgres as a native Windows service, so the two applications on one box genuinely have different runtimes, and a single machine-wide supervisor cannot describe both. `Supervisor::current()` picking one answer for the machine is the assumption that breaks.

A machine subject has no runtime at all: `MachineCx` carries no such field, because machine checks read the host directly.

## Neighbouring cards

`E2` (discover every Postgres cluster) rests on a premise this card's dispatch change is needed to make true. E2 says the machinery for several clusters "is present and tested, it just never gets handed more than one" — but a registry entry running once per subject still receives the same sweep-wide context each time, so handing it several clusters today would report one cluster's readings under every cluster's key. E2 either waits for the per-subject context or builds it itself.

`M1` (DB checks serialise on one shared connection) lands in the same place. `AppCx` carries the connection for the application it was built for, so deciding whether each application gets its own connection or draws from a pool is a decision this card's context split forces rather than one M1 can settle separately.

`J1` (detect and restart PM2 on Windows) overlaps the Windows substrate: "is PM2 running as a service" is exactly what a PM2 runtime has to answer before it can list any services.

`N1` (per-check timing) touches the same dispatch loop, so it is cheaper landed with or right after the context split than before it.

`X1` (decide bestool-canopy's role post `bes-canopy-api`) was answered by `Y1` and looks stale.

## Build steps

Ordered so each step lands on its own. The crate move comes first because it decides where everything else is written, and the context split second because the substrate has nowhere to hang until it exists.

- [ ] Move the daemon into `bestool`: `daemon`, `http_server`, `tasks`, `backup`, `child_confinement`, `windows_service`, `context`, `metrics`, `commands` and `doctor/task.rs`, taking the major bump and flattening `doctor::checks::all()` to `checks::all()` in the same one
- [ ] Split the check signature into machine and application arms, folding the scope and the heal into each, and build the context per subject
- [ ] Key heal's rate limit and in-flight guard on the qualified name
- [ ] Retire the registry's category axis and `is_tamanu`, and restate `has_install` as a property of the application rather than a per-check gate
- [ ] Introduce the substrate trait and the check-storage trait, with own-system implementations
- [ ] Port the duty vocabulary, replacing supervisor unit-name matching in `tamanu_service` and `version_drift`
- [ ] Add per-service resource metrics, graded only against a declared ceiling
- [ ] Take the Postgres tuning check's denominator from the running service's declared ceiling, falling back to the hosting machine's memory
- [ ] Scope check storage per subject, retiring the fixed cache path `http_errors` and `external_users` share
