# Checks and facts split by subject

Verifies that every check and every reported fact is filed against the subject it is
actually about, and that the push carries the split format.

## Subjects and scope

- [x] A machine-scoped check admits only the machine, never an application (verifies spec: SUBJ)
- [x] An application-scoped check never admits the machine (verifies spec: SUBJ)
- [x] A Tamanu-scoped check is not filed against a bare Postgres, but a database-scoped one is (verifies spec: SUBJ)
- [x] Central and facility scopes are mutually exclusive (verifies spec: SUBJ)
- [x] A central-only check has no subject on a facility, so it does not run there (verifies spec: SUBJ)
- [x] With no application on the host, an application check is absent from the results and the wire rather than reported as skipped (verifies spec: SUBJ)
- [x] A machine check still runs on a host with no application (verifies spec: SUBJ)
- [ ] On a real facility host, the 13 central-only checks appear nowhere in the render or the push
- [ ] A machine that hosts no application pushes `machine` with no `applications` key

## Naming and selection

- [x] A name is unique within its subject across the whole registry (verifies spec: SUBJ)
- [x] A bare `--check` name is rejected, and the error names the qualified forms available (verifies spec: DOC)
- [x] A bare unknown name is rejected with guidance that names are qualified (verifies spec: DOC)
- [x] A qualified name for a subject that does not hold that check is rejected (verifies spec: DOC)
- [x] A correctly qualified name is accepted (verifies spec: DOC)
- [x] The CLI renders a check by its instance identity, so two same-named checks are distinguishable

## Postgres as its own application

- [x] The four database checks are scoped to Postgres, not Tamanu (verifies spec: SUBJ)
- [x] Postgres and Tamanu appear as two separate applications with their own types and checks (verifies spec: SUBJ)
- [x] The Postgres application reports the server version and the Tamanu application does not (verifies spec: SUBJ)
- [x] A cluster is keyed by port, and two ports are two applications (verifies spec: SUBJ)
- [x] A cluster reached over a Unix socket resolves to the same key as TCP on that port (verifies spec: SUBJ)
- [x] A remote cluster is keyed apart and never claims the host- prefix (verifies spec: SUBJ)
- [x] localhost, loopback, and socket connections all resolve as local (verifies spec: SUBJ)
- [x] A registry entry runs once per admitting instance, so two clusters give two results
- [x] Both clusters are reached by the one type-level name postgres:connect
- [ ] A real machine with two live clusters reports both, with the right version against each
- [ ] An in-place major upgrade leaves the key unchanged across the upgrade

## Selection contract

- [x] The CLI accepts a qualified name the sweep accepts, and rejects a bare one the same way (verifies spec: DOC)
- [x] `--skip` rejects a bare name on the same terms as `--check` (verifies spec: DOC)
- [x] The sweep announces its plan before any result, so every check shows pending from the start (verifies spec: DOC)
- [x] A planned name matches the identity of the result it will receive
- [x] A result fills its planned row rather than adding a second one
- [x] Two clusters' checks of one name are two rows, identified by instance
- [x] A result outside the plan is kept rather than dropped
- [ ] `--check postgres:connect` on a two-cluster machine runs the check on both
- [ ] The daemon-streamed path shows pending rows from the daemon's plan on a real host

## The split payload

- [x] Checks land in their own subject's `health[]` and no other (verifies spec: SUBJ)
- [x] An application reports no `bestoolVersion` and no hostname (verifies spec: SUBJ)
- [x] The machine reports `bestoolVersion` and hostname (verifies spec: SUBJ)
- [x] The clock timezone is the machine's and the configured timezone the application's, neither carrying the other's (verifies spec: SUBJ)
- [x] A check's lifted `payload_extras` land on that check's subject (verifies spec: SUBJ)
- [x] The push names its source as `alertd` (verifies spec: SUBJ)
- [x] The top-level flat `health[]` is sent empty
- [x] An off-wire check stays out of its subject's `health[]`
- [x] A skip reaches the wire as `skipped` rather than as a failure
- [x] The application entry carries its type slug
- [ ] A live push against a canopy staging instance is accepted and shows both grains

## Review fixes

- [x] An unsplit canopy response still governs application checks, so a failure is not masked as a warning
- [x] A split response leaves an application canopy does not hold to the absent-check default
- [x] An unparseable connection string still reports a cluster, so `connect` can alert
- [x] A mixed host list names the host that is actually remote
- [x] Every check reaches the wire even when its details would not deserialise
- [x] A check's details, summary and reason all reach the wire entry
- [x] `tuning` and the subject key agree about whether a cluster is local
- [ ] A live push with a check carrying unusual detail keys is accepted by canopy
- [x] The subject resolution compiles and its tests hold on Windows, where Unix sockets do not exist

## Severities

- [x] The machine's ceilings and an application's are applied separately to checks of the same bare name (verifies spec: SUBJ)
- [x] Each target's wire `health[]` tracks its own capped status
- [x] A check canopy has not heard of defaults to a warn ceiling
- [x] The ungrouped severities endpoint's flat map governs every subject
- [ ] A canopy response with no `machine` target falls back to the top-level severities map

## Overall result

- [x] A failing application makes the whole sweep failing
- [x] A sweep is healthy when every subject passes

## Machine identity

- [x] The identity resolves from the standard file when present
- [x] A fresh identity is minted and persisted when absent, and is stable across runs
- [x] An unwritable path surfaces as an error naming the machine id
- [ ] A canopy registration's `server_id` still takes precedence over the file

## Daemon and CLI round-trip

- [x] The cached payload reconstructs machine checks with the machine subject
- [x] The cached payload reconstructs both grains, keeping each check's subject
- [x] A payload describing no targets reconstructs to no results
- [ ] A live daemon sweep read back through `doctor` renders the same checks the daemon computed
- [ ] The streamed recompute path carries each check's subject through to the CLI
