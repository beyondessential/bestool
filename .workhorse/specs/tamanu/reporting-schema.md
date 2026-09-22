---
id: CHK-RSC
---

# Reporting schema

A reporting schema is the set of database views a Tamanu server's reports read from.
Canopy offers one per group and Tamanu version.
bestool's part is to say which schema this server has, and to apply the one it is offered where they differ.

Every Tamanu server in a group carries one, central and facility alike, since a facility serves reports of its own.
A group's schema is the same on all of them.

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

Only a schema whose bytes Canopy holds is taken as an offer, which is what the offer naming Canopy itself says.
A schema artifact resting somewhere Canopy records rather than holds belongs to no group, is offered to every server in the fleet, and is passed over; a digest may be registered with one of those as readily as with a held one, so carrying a digest says nothing about which it is.
The digest it does carry has to be one bestool can check the bytes against, a sha256 SRI: an offer named with any other algorithm would grade the server as behind and then be refused on every apply.
The digest also names which build of a version a schema is: a group gets a new build of the version it already runs whenever its reports are fixed, so a server on an earlier build of the offered version is graded as needing the newer one.

The check passes when the stamp matches what is offered.
It fails when they differ, when the server's schema carries no stamp, and when the server has no schema at all and one is offered.

It skips when Canopy offers none for this version, since a pair Canopy has not built is Canopy's finding to raise rather than this server's fault.
It skips when Canopy is unreachable, still reporting the stamp: whether a schema is the right one is Canopy's to answer, and an unreachable Canopy is not this server's failing.
It skips where the database is unreachable. A host with no Tamanu carries no Tamanu application, so the check is absent there rather than skipping.

## Applying it

Applying the offered schema is the check's self-heal action, so it runs only in the long-running daemon and only while the check is failing.

The schema's own SQL drops the schema and recreates it, and is applied as one batch, so a statement that fails partway leaves the server the schema it already had.
That holds only while the artifact carries no transaction control of its own: a `COMMIT` part-way through ends the batch's transaction, and a later failure then leaves the server with neither the schema it had nor the one offered.
A schema artifact therefore carries no `BEGIN`, `COMMIT` or `ROLLBACK`, nor the `START TRANSACTION`, `END` and `ABORT` Postgres takes for the same three, and one that does is refused rather than applied.

The bytes fetched are checked against the digest Canopy offered before any of them reach the database, and a schema that is not the one Canopy named is refused.

A failed apply is logged and retried under the heal backoff.
An artifact that applied without stamping the offered version is not applied again, since retrying it would rebuild the schema on every backoff step for as long as the offer stands.
