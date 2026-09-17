# K1 test cases

Scenarios verifying that a check reads its application's runtime through a substrate, grades it with logic that does not vary by substrate, and remembers readings against its own subject.

## The two contexts

- [x] A Postgres cluster's context carries no product version, install root or configuration, because it has no field for them (verifies spec: SUBJ)
- [x] A check written against a cluster's context cannot reach a Tamanu's, and the reverse — enforced by `Run`'s shape rather than by a test
- [x] Every registry entry is filed under the arm matching the subject `SUBJ` gives it (verifies spec: SUBJ)
- [x] A heal sits in the same arm as its check, so it is handed the context the check ran with (verifies spec: CHK#self-healing)

## Reading the runtime

- [x] One check grades identical readings to the same outcome whichever runtime served them (verifies spec: SUB)
- [x] A reading a runtime cannot serve skips the check, carrying the runtime's reason rather than a generic one (verifies spec: SUB)
- [ ] A check that does not apply to a subject is absent from its report, and is not confused with a reading that could not be taken (verifies spec: SUB)
- [x] A cluster's context offers no traffic or certificate reading at all, rather than one that answers unavailable (verifies spec: SUB)

## One runtime per application

- [x] Two applications on one machine each resolve their own runtime (verifies spec: SUB)
- [ ] A Tamanu under a process supervisor and a Postgres under a native service on one machine each read through their own runtime (verifies spec: SUB) — needs a Windows host
- [ ] A machine subject resolves no runtime, and machine checks read the host directly (verifies spec: SUB)

## Duties

- [x] A check reads a service's duty, never its unit, process or pod name (verifies spec: SUB)
- [x] The same duty is named identically whichever runtime reported it (verifies spec: SUB)
- [x] A service whose duty is outside the vocabulary is reported under the name it was found by, rather than dropped (verifies spec: SUB)
- [x] A deployment shape that should no longer exist is found as an out-of-vocabulary service and graded as forbidden (verifies spec: SUB)
- [x] The service-expectation logic grades a shortfall in running services the same way on every runtime (verifies spec: SUB)

## Resource usage per service

- [x] Each service's memory and processor usage are reported as metrics, dimensioned by duty and service (verifies spec: SUB)
- [x] Usage is reported whether or not anything grades it (verifies spec: SUB)
- [x] A service with a declared ceiling is graded against that ceiling (verifies spec: SUB)
- [x] A service with no declared ceiling reports usage and skips the grading, rather than being graded against the machine's total (verifies spec: SUB)
- [ ] A ceiling is read from a container limit and from a supervised unit's configured memory bounds alike (verifies spec: SUB)

## Postgres tuning

- [ ] The tuning check reports for the Postgres application, not for whatever uses it (verifies spec: SUBJ)
- [x] Settings are graded against the declared ceiling of the service running the cluster where one exists (verifies spec: SUB)
- [ ] With no declared ceiling, the denominator is the hosting machine's memory (verifies spec: SUB)
- [x] With neither a ceiling nor a hosting machine, the check skips rather than inventing a denominator (verifies spec: SUB)

## Check storage

- [x] A check's store is scoped to its subject, so two applications' histories never meet (verifies spec: SUB)
- [x] Two applications running one stateful check each read back only what they themselves wrote (verifies spec: SUB)
- [x] The fixed cache path `http_errors` and `external_users` shared is gone (verifies spec: SUB)
- [x] State written as lasting only until the compute restarts is dropped when a sweep observes the compute off (verifies spec: SUB)
- [x] State written as durable survives that same sweep (verifies spec: SUB)
- [ ] A check waking after a sleep computes no delta against a baseline taken before it (verifies spec: SUB)

## Compute switched off

- [ ] An application with its compute off reports no running services and is not graded as failing (verifies spec: SUB)
- [ ] Checks needing a running service or a live database skip, naming that state as the reason (verifies spec: SUB)
- [ ] Facts drawn from declared configuration are still reported while it sleeps (verifies spec: SUB)

## Traffic readings

- [ ] Traffic statistics are asked for per application, not per machine (verifies spec: SUB)
- [ ] Where one reading serves several applications, it is filtered to the application being reported for (verifies spec: SUB)
- [ ] History is kept per source, and a source that has vanished is dropped rather than graded as the quantity having fallen (verifies spec: SUB)

## Nothing else moved

- [ ] Every check's outcome for a given reading is unchanged from before the substrate was introduced
- [ ] The workspace builds and its tests pass on Linux and on a Windows target
