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

### Still open

Whether `bestool-alertd` splits into a checks-and-machinery library and a daemon that is one consumer of it. `CHK` states the property — the checks and the machinery are available independently of the daemon that schedules them — without settling the crate layout that delivers it. Larger than anything on this card, and worth deciding before the substrate trait is written, since it determines where that trait lives.

### Cleanup this enables

`CheckContext` still carries `has_install` and `is_tamanu`. `CheckScope` made the second redundant — a non-Tamanu database is a Postgres application, which `CheckScope::Tamanu` does not admit — so it can go. `has_install` remains a real distinction (an application known only through its database has no install files to read) but is a property of the application rather than a gate each check consults.

## Build steps

- [ ] Introduce the substrate trait and the check-storage trait, with own-system implementations
- [ ] Retire `is_tamanu`, and restate `has_install` as a property of the application rather than a per-check gate
- [ ] Port the duty vocabulary, replacing supervisor unit-name matching in `tamanu_service` and `version_drift`
- [ ] Add per-service resource metrics, graded only against a declared ceiling
- [ ] Take the Postgres tuning check's denominator from the running service's declared ceiling, falling back to the hosting machine's memory
- [ ] Scope check storage per subject, retiring the fixed cache path `http_errors` and `external_users` share
