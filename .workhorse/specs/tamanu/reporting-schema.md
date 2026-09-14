---
id: CHK-RSC
---

# Reporting schema

A reporting schema is the set of database views a Tamanu server's reports read from.
Canopy offers one per group and Tamanu version.
bestool's part is to say which schema this server has, and to apply the one it is offered where they differ.

It is one of the healthchecks described by [CHK](healthchecks.md) and follows the shared outcome model in `tamanu/doctor.md`.

## What the server has

A reporting schema stamps its version into the database when it is applied, as the comment on the `reporting` schema itself.
bestool reads that comment and parses it as a version.

A server with no `reporting` schema reads as having none.
A `reporting` schema whose comment is missing, or is not a version, reads as unstamped, and no version is reported for it.

## Reporting it

The stamp is reported to Canopy as a top-level status fact.
It is reported whether or not the schema matches what Canopy offers.

## Grading it

Canopy is asked what it offers for the version this server runs, over the authenticated connection: a schema belongs to a group, and Canopy answers for the caller's group.
Canopy resolves which artifact a version is offered, so the schema in its answer is the one graded against.

The check passes when the stamp matches what is offered.
It fails when they differ, when the server's schema carries no stamp, and when the server has no schema at all and one is offered.

It skips when Canopy offers none for this version, since a pair Canopy has not built is Canopy's finding to raise rather than this server's fault.
It skips when Canopy is unreachable, still reporting the stamp: whether a schema is the right one is Canopy's to answer, and an unreachable Canopy is not this server's failing.
It skips on a host with no Tamanu, and where the database is unreachable.

## Applying it

Applying the offered schema is the check's self-heal action, so it runs only in the long-running daemon and only while the check is failing.

The schema's own SQL replaces the schema wholesale, so applying it needs no additional reconciliation.
A failed apply is logged and retried under the heal backoff, and leaves the previous schema as it was.
