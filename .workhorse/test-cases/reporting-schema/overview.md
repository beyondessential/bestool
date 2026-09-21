# Reporting schema (CHK-RSC) — test sweep

bestool#868, `feat/reporting-schema-apply` at `9398cae5`. Spec:
`.workhorse/specs/tamanu/reporting-schema.md`.

Suite state on the branch: `cargo test -p bestool-alertd --lib` is 463/463 against a
populated `tamanu-central`; `cargo test -p bestool-canopy --all-features` is 62/62;
`cargo test -p bestool --lib canopy_contract -- --ignored` is 15/15. The five
`runs_against_central` checks fail on an empty `tamanu-central` and pass on a real one,
on this branch and on main alike, so an empty local database is the cause rather than
anything here.

## Covered

- [x] No database connection skips, and carries no version fact — CHK-RSC
- [x] `STAMP_SQL` reads back the three states the check branches on (no schema, a schema
      with no comment, a stamped schema) against a real Postgres — CHK-RSC
- [x] A batch that fails partway leaves the schema that was already there — CHK-RSC
- [x] Canopy absent grades nothing and says so in the summary — CHK-RSC
- [x] Only an artifact of type `reporting-schema` is taken as the schema — CHK-RSC
- [x] A 404 from the artifacts listing is nothing offered; 401, 403, 500, 502 and 503
      are the ask failing — CHK-RSC
- [x] 500, 502, 503 and a transport error are canopy being out; 401, 403, 404 and a body
      that would not decode are answers — CHK-RSC
- [x] A stamp equal to the offered version passes — CHK-RSC
- [x] A stamp naming another version fails, and the summary names both — CHK-RSC
- [x] No schema at all fails — CHK-RSC
- [x] An unstamped schema fails with a summary distinct from the absent one, and reports
      no version fact — CHK-RSC
- [x] The version fact rides a failure as well as a pass — CHK-RSC
- [x] `v2.60.0` and `2.60.0+build7` both name the schema `2.60.0` names — CHK-RSC
- [x] Blank, over-length, non-version and markup-bearing comments read as no stamp — CHK-RSC
- [x] Only a digest-shaped build is carried off the comment — CHK-RSC
- [x] A download URL naming any origin but canopy's is refused — CHK-RSC
- [x] A download URL on canopy's origin is followed for its path — CHK-RSC
- [x] A redirect is refused rather than followed — CHK-RSC
- [x] A GET canopy answers itself comes back — CHK-RSC

## Needs a fix

- [ ] **The check runs, and heals, on facility servers.** `crates/alertd/src/checks.rs:573`
      registers it as `tamanu_app`, which is `TamanuScope::Any`; `central` exists and is
      what other central-only checks use. Canopy does not narrow it either: `caller_scope`
      resolves to the calling machine's `group_id` with no server-kind filter, so a facility
      in the group is offered the group's schema. Demonstrated locally: the real 128-view
      `v2.60.2` build applies cleanly to a facility database (all 128 views created, stamp
      set, no errors), because facility and central share one migration set and differ by a
      single table. So a facility grades fail, heals, applies, then grades pass, and every
      facility in the group silently carries the central's reporting schema. Register it
      `central`.
- [ ] **Nothing enforces the artifact carrying no transaction control.** The spec makes it
      a precondition of the atomicity guarantee and neither side checks it. Demonstrated:
      with a `COMMIT` in the body, a later failure commits the drop and leaves `reporting`
      created, empty and unstamped, with reports broken until a later apply succeeds. A
      guard before `batch_execute` makes the precondition true rather than assumed.
- [ ] **The fetched bytes are never compared against `offered.digest`.** `fetch_offered`
      checks media type, size and UTF-8, then `stamped()` writes the digest into the schema
      comment as though it had been verified. Canopy verifies its held bytes before serving
      and the transport is origin-pinned with redirects refused, so this is defence in depth
      rather than a hole, and the digest is already in hand.

## Automatable, not yet written

- [ ] **Grading on the build.** `grade` with `(Some(build), Some(digest))` equal passes,
      and differing fails with "a newer build offered". The test helper builds every
      `Offered` with `digest: None`, so the behaviour `9398cae5` exists for is asserted
      nowhere. Unit test beside the existing gradings.
- [ ] `stamped()` appends `COMMENT ON SCHEMA reporting IS '<version> <digest>'`, returns
      the SQL untouched where there is no digest, and `quoted()` doubles an apostrophe.
      Unit.
- [ ] `fetch_offered` refuses a media type outside `SCHEMA_MEDIA_TYPES`, a declared length
      over `MAX_SCHEMA_BYTES`, a streamed body that crosses it, and an empty body. The
      transport tests' `serve_once` is the fixture.
- [ ] The offer cache: a second ask inside `OFFER_TTL` does not reach the wire, and a
      different version asks afresh. Unit.
- [ ] `applied_without_stamping` blocks re-applying the same artifact and digest, and does
      not block a rebuild that re-registers under the same id with a new digest. Unit.
- [ ] `apply_offered` end to end against a real Postgres and a `serve_once` canopy:
      returns `Healed` where the stamp reads back, and `Failed` plus a note where the
      applied SQL stamps something else.
- [ ] `apply_connection` opens through `connect_one` and sets both timeouts, so a
      `sslmode=require` URL works where the shared pool does.

## E2E

None. No browser journey in this change.

## Not automated

Two flows. Everything else here is reachable from a test.

- [ ] **A server takes an offer from a real canopy and applies it unattended.** Nothing
      automated builds a `CanopyClient` against a real canopy, presents a device
      certificate, or runs the daemon's sweep-then-heal loop; the unit tests grade
      in-process with a hand-built `Offered`. Needs a registered machine in a group with a
      published artifact, and the group scoping is only real when a second group exists.
- [ ] **The apply against a populated central under live report traffic.** A test database
      has no concurrent readers, and the question is how long reports are unavailable
      during a rebuild and whether `lock_timeout` leaves the old schema intact when a
      report is mid-query. The mechanism is verified locally against a real central; what
      a human adds is real volume and real concurrent load.

## Run log

- Suite run on a populated `tamanu-central` loaded from a dump of `tamanu-central-local`,
  2026-09-22. The 5 `runs_against_central` failures on an empty database reproduce
  identically on `origin/main`.
- The apply, the facility apply and the lock-contention behaviour were exercised by hand
  against throwaway copies of the local central and facility databases using the real
  `v2.60.2` standard build. No test covers any of the three.

## QA document

`qa.html` beside this file, published at
https://claude.ai/artifact/LdqEQMTBETcqFS2YLwNirY (icon `checklist`). Three smoke
checks and the two flows above, with both fixtures and the timings a tester waits on.
Results go on the Linear card as `🤖 ✅ TEST_n` comments.

## Smoke run, 2026-09-22

Ubuntu 25.04 aarch64 (lima `seedling`), Postgres 17.7, bestool 2.1.4 cross-built from
this branch for `aarch64-unknown-linux-musl`, against a real central schema loaded from
a dump and `TAMANU_DATABASE_URL` set.

- SMOKE_1 pass. The check is registered and returns a result.
- SMOKE_2 not runnable. The VM has no canopy registration, so the check skips
  `canopy unreachable` rather than `none offered for this version`. The offline path is
  exercised; the reachable-canopy branch still needs a registered server.
- SMOKE_3 pass. Fixture A applies on Linux and Postgres 17, the stamp reads back as
  `2.60.0 sha256-LCTbqpIiSOs=`, and the check publishes `reportingSchemaVersion` of
  `2.60.0` under the application's `detail`, carrying the version without the digest.
  A comment of `built by hand` publishes no fact at all.

The facility scope shows up from the CLI without any of this: `--check reporting_schema`
is refused with `needs the subject it reports for: tamanu-central:reporting_schema,
tamanu-facility:reporting_schema`. A URL-only deployment resolves as
`tamanuServerKind: central`.
