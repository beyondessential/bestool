# Reporting schema (CHK-RSC) — test sweep

bestool#868, `feat/reporting-schema-apply` at `0360e74d`. Spec:
`.workhorse/specs/tamanu/reporting-schema.md`.

Suite state: `cargo test -p bestool-alertd --lib` is 486/486 against a populated
`tamanu-central`, of which 41 are this check's. `cargo test -p bestool-canopy
--all-features` is 65/65. `cargo test -p bestool --lib canopy_contract -- --ignored`
is 15/15.

`tamanu-central` has to exist and carry a real Tamanu schema or five unrelated
`runs_against_central` checks fail, on this branch and on main alike. CI creates no
such database, so those five and two of this check's never run there.

## Covered

Grading and the stamp:

- [x] A stamp equal to the offered version and build passes — CHK-RSC
- [x] A stamp naming an earlier build of the offered version fails as a newer build
      offered, and one naming no build at all does the same — CHK-RSC
- [x] A stamp naming another version fails, and the summary names both — CHK-RSC
- [x] No schema at all fails; an unstamped schema fails with a distinct summary and
      reports no version fact — CHK-RSC
- [x] `v2.60.0` and `2.60.0+build7` name the schema `2.60.0` names — CHK-RSC
- [x] Blank, over-length, non-version and markup-bearing comments read as no stamp,
      and only a digest-shaped build is carried off one — CHK-RSC
- [x] The version fact rides a failure as well as a pass, and absent where there is
      no version to report — CHK-RSC
- [x] `STAMP_SQL` reads back the three states the check branches on, against a real
      Postgres — CHK-RSC

What canopy offers:

- [x] Only an artifact of type `reporting-schema` is taken as the schema — CHK-RSC
- [x] An artifact of that type resting anywhere but canopy is passed over, carrying a
      digest or not, as is one named with a digest the fetch could not check the bytes
      against — CHK-RSC
- [x] A 404 is nothing offered; 401, 403 and 5xx are the ask failing — CHK-RSC
- [x] 5xx and transport errors are canopy being out; 401, 403, 404 and a body that
      would not decode are answers — CHK-RSC
- [x] An answer already given inside the TTL is not asked for again, and an upgrade
      asks afresh — CHK-RSC
- [x] No database connection skips, and canopy absent grades nothing — CHK-RSC

Fetching and applying:

- [x] A download URL naming any origin but canopy's is refused, a redirect is
      refused, one carrying an authority in its path is refused, and one on canopy's
      origin is followed for its path and query — CHK-RSC
- [x] A media type outside the list, a declared length over the ceiling, a body that
      streams past it, and an empty body are each refused — CHK-RSC
- [x] Bytes that are not the ones canopy named are refused before any reach the
      database — CHK-RSC
- [x] An artifact carrying `BEGIN`, `COMMIT`, `ROLLBACK`, `START TRANSACTION`, `END`
      or `ABORT` in statement position is refused, and the same words elsewhere are
      not — CHK-RSC
- [x] The offered build is stamped onto the schema, an artifact ending without a
      terminator is stamped all the same, and an apostrophe in a digest cannot close
      the literal — CHK-RSC
- [x] A batch that fails partway leaves the schema that was already there — CHK-RSC
- [x] An apply leaving the offered stamp heals, against a real Postgres and a canopy
      answering over HTTP — CHK-RSC
- [x] A rebuild re-registering under one artifact id is not held back by the build it
      replaces — CHK-RSC
- [x] The apply opens a bounded connection of its own, with both timeouts set — CHK-RSC

## Decisions recorded

- [x] **The check runs on facilities as well as centrals.** A facility serves reports
      of its own (`packages/facility-server/app/routes/apiv1/reports.js:34` reads
      `reportSchemaStores`) and carries the reporting roles, so it is offered and
      applies the group's schema like the central does. Central and facility run one
      shared migration set with no migration branching on server kind, so a schema
      built for the group applies on either. Demonstrated: the real 128-view `v2.60.2`
      build applies cleanly to a facility database. Now in the spec so it is not
      "fixed" again.
- [x] **Canopy already offered a `reporting-schema` artifact before this pipeline.**
      Read from production canopy 2026-09-22: 2.54.x, 2.57.x, 2.61.x and 2.63.x each
      carry an unscoped range artifact of that type, pointing at a release bucket,
      with no digest. Unscoped artifacts are offered to every machine, so without a
      filter every server on a published version would have graded fail, tried to
      heal, and had the fetch refused for naming a non-canopy origin: a permanently
      red check fleet-wide. Closed by taking only an artifact canopy holds, which is
      the offer naming canopy itself: canopy's own `ART#digests` has an unscoped
      artifact carrying a digest for whoever fetches it to check, so a digest says
      nothing about where the bytes rest. The legacy artifacts are still in canopy and
      are now passed over rather than acted on.
- [x] **The group-scoped answer holds on the tailscale path too.** The `/public` mount
      names where the public routes are mounted inside the private-server, not an
      unauthenticated surface: the tailscale ingress terminates client TLS, so canopy
      resolves the caller's tailnet address to a node identity and keys that into
      `devices.tailscale_node_id`, and the group follows from the device's machine as
      it does under mTLS (`commons-servers/src/device_auth/mod.rs`). A caller it
      cannot resolve is offered the unscoped artifacts alone rather than another
      group's, and those are now passed over.
- [x] **The read-back mismatch branch is now unreachable through the apply.** The
      stamp is appended to the artifact's SQL in the same atomic batch, so a batch
      that succeeds always leaves a matching stamp. The `unstamped` registry behind it
      is still covered at unit level. Keeping it as insurance is a judgement call left
      open rather than settled.

## Automatable, not yet written

- The apply connection asks for no password. Reaching the prompt needs a URL whose
  auth fails with no password in it, which the local databases do not give.
- The heal takes nothing from the checks' pool while it downloads and applies.
  Structural, and only observable by instrumenting the pool.

## E2E

None. No browser journey in this change.

## Not automated

One flow, and it is blocked. See the run log.

- [ ] **A server takes an offer from a real canopy and applies it unattended.**
      Nothing automated presents a device certificate to a real canopy or runs the
      daemon's sweep-then-heal loop; the tests drive an in-process client against a
      local HTTP server. Group scoping is only real once a second group exists to be
      excluded.

The apply under live report traffic, which this sweep previously listed as a second
manual flow, is now covered as far as a person can add anything: the mechanism is
verified against a real central, and what a human would add is volume and concurrency
rather than a different assertion.

## Run log

- Suite run on a populated `tamanu-central` loaded from a dump of
  `tamanu-central-local`, 2026-09-22. The five `runs_against_central` failures on an
  empty database reproduce identically on `origin/main`.
- The apply, the facility apply and the lock-contention behaviour were exercised by
  hand against throwaway copies of the local central and facility databases using the
  real `v2.60.2` standard build, before the tests existed. All three are now covered.
- **Enrollment cannot be faked locally.** `bestool canopy register` takes an encrypted
  ticket whose `api_url` must be `https`, and the client's base URL comes from the
  registration with no override, so a plain-HTTP local canopy cannot be enrolled
  against. That is why the grading half was run on already-enrolled demo servers
  instead.
- **TEST_1 is staged up to the apply.** Both demo servers carry a branch build at
  `/tmp/bestool-rsc`, the central's schema is captured, and the grading half is proven.
  What remains is registering a held artifact for exactly 2.54.14 against group
  `0bc62657` with an admin credential, then running the branch build as the daemon
  briefly so heal fires. The apply replaces the central's 241 views with the 2.54.x
  build's 167, which is why it stops here for a human.

## Smoke run, 2026-09-22

Ubuntu 25.04 aarch64 (lima `seedling`), Postgres 17.7, bestool cross-built from this
branch for `aarch64-unknown-linux-musl`, against a real central schema loaded from a
dump with `TAMANU_DATABASE_URL` set. Re-run on the merged branch at bestool 2.2.0.

- SMOKE_1 pass. The check is registered and returns a result, for both the central and
  the facility subject.
- SMOKE_2 pass, against production canopy on a registered demo server rather than in
  the VM. `none offered for this version` on a box running 2.54.14, for which canopy
  does hold a `reporting-schema` artifact: the unscoped range one pointing at the
  release bucket. That answer is only reachable because a schema canopy holds nothing
  for is passed over, so it demonstrates the filter end to end on production data. In
  the VM, with no registration, only the `canopy unreachable` path runs.
- SMOKE_3 pass. Fixture A applies on Linux and Postgres 17, the stamp reads back, and
  the check publishes `reportingSchemaVersion` of `2.60.0` under the application's
  `detail`, carrying the version without the digest. A comment of `built by hand`
  publishes no fact at all.

`--check reporting_schema` is refused with `needs the subject it reports for`, naming
both subjects. A URL-only deployment resolves as `tamanuServerKind: central`.

## Findings from the demo-server run, 2026-09-22

Read-only baseline on `inventory-test-central` and `inventory-test-facility-a`, both
Ubuntu 24.04 aarch64 on Tamanu 2.54.14, alertd live and enrolled.

- **A one-shot `doctor` could not grade this check at all.** The local sweep passed no
  canopy client, so it always skipped with `canopy unreachable`, on a correctly
  registered server with canopy reachable. Only the daemon graded it, which meant the
  only way to see a grade was to run a daemon and accept that it would also heal.
  Fixed in `2091b98c`: the client is built once and shared with the severities fetch,
  healing still off, so a local sweep only ever reads.
- **The reporting role on 2.54 is `reporting`, not `tamanu_reporting`.** Tamanu owns
  the newer roles from 2.60. A fixture granting to `tamanu_reporting` fails on a 2.54
  box, and the real 2.54.x build grants to `reporting`.
- **The central already carries a 241-view `reporting` schema with no comment**, so it
  grades unstamped. The facility carries the schema with no views. The real 2.54.x
  build produces 167 views, so applying it is a real change rather than a no-op. The
  central's current definition is captured at `/tmp/reporting-before.sql` on the box
  and locally in this session's scratchpad.

## QA document

`qa.html` beside this file, published at
https://claude.ai/artifact/LdqEQMTBETcqFS2YLwNirY (icon `checklist`). Results go on the
Linear card as `🤖 ✅ TEST_n` comments.
