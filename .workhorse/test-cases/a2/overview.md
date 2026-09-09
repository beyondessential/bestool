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
- [ ] `--skip` rejects a bare name on the same terms as `--check`
- [ ] The CLI renders a check by its qualified name, so two same-named checks are distinguishable

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
