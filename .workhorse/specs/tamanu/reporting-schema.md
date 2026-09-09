---
id: CHK-RSC
---

# Reporting schema

A reporting schema is the set of database views a Tamanu server's reports read from.
Part of it follows from the Tamanu version's own database schema and the rest from the group's configuration, so it is built centrally against a replica of that group's data at that version and offered back per group.
bestool's part is to say which schema this server has, and to apply the one it is offered where they differ.

It is one of the healthchecks described by [CHK](healthchecks.md) and follows the shared outcome model in `tamanu/doctor.md`.

## What the server has

The version a reporting schema was built for is stamped on the schema itself by the SQL that built it, so what a server has is read from the server rather than from anything bestool records.
A schema applied by hand therefore reads the same as one bestool applied.

A server with no reporting schema at all reads as having none, which is a finding rather than an error.
A reporting schema carrying no stamp reads as unstamped rather than as absent: something built it that was not this pipeline, so an operator is replacing a schema rather than applying a first one, and no version is reported for it.

## Reporting it

The stamp is reported to Canopy as a top-level status fact, so which schema each server is on is answerable across the fleet without reading into a check's detail.
It is reported whether or not the schema matches what Canopy offers, since a server on the wrong schema is exactly when knowing which one it has matters.

## Grading it

Canopy is asked what it offers for the version this server runs, over the authenticated connection: a schema belongs to a group, and Canopy answers for the caller's group.
Only a schema Canopy published for one exact version is graded against.
A schema registered against a version range is ignored, since a schema follows the migrations one version applies and Canopy resolves a range artifact for every version it covers.

The check passes when the stamp matches what is offered.
It fails when they differ, when the server's schema carries no stamp, and when the server has no schema at all and one is offered.

It skips when Canopy offers none for this version, since a pair Canopy has not built is Canopy's finding to raise rather than this server's fault.
It skips when Canopy is unreachable, still reporting the stamp: whether a schema is the right one is Canopy's to answer, and an unreachable Canopy is not this server's failing.
It skips on a host with no Tamanu, and where the database is unreachable.

## Applying it

Applying the offered schema is the check's self-heal action, so it runs only in the long-running daemon and only while the check is failing.
Applying is the only thing bestool does that writes to Tamanu's database: every check stays read-only, and the interactive doctor command never applies anything.

The schema's own SQL replaces the schema wholesale, so applying it needs no additional reconciliation.
A failed apply is logged and retried under the heal backoff, and leaves the previous schema as it was.
