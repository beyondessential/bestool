# L2 test cases

Scenarios verifying that each check receives a context built for the subject it
reports for, that the two arms are distinct, and that nothing else moved.

## Per-subject dispatch

- [x] A check filed against two Postgres clusters produces one result per cluster, each keyed by its own port (verifies spec: SUBJ)
- [x] A check whose subject the host has no instance of is absent from the sweep, not reported as skipped (verifies spec: SUBJ)
- [x] A machine check runs once, against the machine, whatever applications the host has (verifies spec: SUBJ)
- [x] A central-only check has no subject on a facility and does not run there (verifies spec: SUBJ)
- [ ] Two Postgres clusters on one host report their own readings under their own keys, rather than one cluster's readings under both (verifies spec: SUBJ) — needs a sweep against a host with two clusters, which nothing discovers yet (`E2`)

## The two arms

- [x] Every check is filed under the subject `SUBJ` gives it: machine, Postgres, Tamanu, or central (verifies spec: SUBJ)
- [x] The four database checks are filed against the Postgres application, not against whatever uses it (verifies spec: SUBJ)
- [x] Each check's heal sits in the same arm as the check, so it is handed the context the check ran with (verifies spec: CHK#self-healing)
- [x] The registry's qualified names are unique, so no two entries collide on one `subject:name` (verifies spec: SUBJ)
- [ ] A machine runner paired with an application scope does not compile — enforced by `Run`'s shape rather than by a test

## Heal keying

- [x] Two applications' heals for one check hold their own rate limit and their own in-flight slot (verifies spec: CHK#self-healing)
- [x] A heal attempt in flight refuses a second attempt for the same qualified name (verifies spec: CHK#self-healing)
- [x] A deferred attempt backs off rather than retrying on the next sweep (verifies spec: CHK#self-healing)
- [x] A successful repair still waits its minimum interval (verifies spec: CHK#self-healing)

## Retiring the second axis

- [x] A generic database URL resolves a Postgres application and no Tamanu one (verifies spec: SUBJ)
- [x] With no Tamanu application, every Tamanu-scoped check is absent and machine checks still run (verifies spec: SUBJ)
- [x] A Tamanu URL is preferred over the generic one, and the Tamanu it names has no install root (verifies spec: SUBJ)
- [x] Checks run against an application known only through its database; none is gated out for lack of an install
- [x] `connect` runs and fails against an unreachable database on an application with no install

## Caddyfile version

- [x] The check skips on a non-Windows host (verifies spec: CHK-CFV)
- [x] The check skips on a Windows host with no Tamanu, naming the deployment's version as what it grades against (verifies spec: CHK-CFV)
- [ ] The check skips when no Caddyfile is on disk (verifies spec: CHK-CFV) — needs a Windows host with caddy absent
- [x] Marker outcomes against the Tamanu version are unchanged (verifies spec: CHK-CFV)

## Nothing else moved

- [x] A blocking check does not inflate a sibling's reported latency (verifies spec: CHK#concurrent-execution)
- [x] Concurrent checks each take their own database backend (verifies spec: CHK#concurrent-execution)
- [x] A host with no reachable database has no pool and DB checks skip rather than waiting out an acquire
- [x] Each application's detail comes from its own fact block (verifies spec: SUBJ)
- [x] The daemon re-resolves its targets per sweep and keeps the last known one when discovery errors
- [ ] A full sweep on a real host reports the same checks, outcomes and wire payload as before the split — manual
