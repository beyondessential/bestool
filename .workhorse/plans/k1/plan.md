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

## Open: is a uniform substrate the right shape?

Working through A2 suggests it may not be, and A2's own implementation strengthens the case.

Three of the four things `SUB` has a substrate answering for are not substrates: a database connection, Tamanu's config and version, and "am I the machine" are parameters and a boolean. Only the workload grouping — duties, services, per-service facts — abstracts genuinely different acquisition. The other three restate the old `@tamanu` / `@db` / `host` categories.

The suite is the bigger issue. 18 of 45 checks are machine checks and a process observing applications remotely assembles none of them, so a large part of the catalogue would exist only to report inapplicability.

The alternative: expose the checks and the shared machinery, and let a consumer assemble the suite it needs, rather than running alertd as one API with a substrate plugged into it.

A2 has already built the part of that which matters most. `CheckScope` is the registry-declares-its-own-requirements model, filtered per subject, rather than hand-assembly by check name — so a new check propagates by default and exclusion is deliberate. And the applicability-versus-skipped rule it specced is the distinction that model needs: absence means "does not belong to this subject", skip means "belongs here but could not be read on this sweep".

What that leaves:

- **Still needed**: the abstraction over how a reading that feeds a threshold is obtained. Assembly owns *which* checks run; if a consumer supplies the numbers instead of the check asking for them, two consumers can grade against different denominators. The seam narrows rather than disappears, and the workload grouping is where it survives.
- **Preserved**: the property that two environments cannot diverge into subtly different checks, because the shared unit is the check itself rather than the whole sweep.
- **Kept regardless**: the duty vocabulary, per-service metrics, and check storage as an injectable.
- **Raised**: whether `bestool-alertd` splits into a checks-and-machinery library and a daemon that is one consumer of it. Larger than anything currently on this card.

## Build steps

- [ ] Introduce the substrate trait and the check-storage trait, with own-system implementations
- [ ] Port the duty vocabulary, replacing supervisor unit-name matching in `tamanu_service` and `version_drift`
- [ ] Add per-service resource metrics, graded only against a declared ceiling
- [ ] Take the Postgres tuning check's denominator from the running service's declared ceiling, falling back to the hosting machine's memory
- [ ] Scope check storage per subject, retiring the fixed cache path `http_errors` and `external_users` share
