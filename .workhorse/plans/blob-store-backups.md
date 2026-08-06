# Plan: blob store backups

Tamanu is moving attachment and asset bytes out of the database into a
content-addressed blob store on disk.
Once it does, a `tamanu-postgres` backup no longer captures a server's whole state: the database holds references and the store holds the bytes, and the two have to be backed up and restored consistently.

The behaviour is specified on the Tamanu side, in `specs/blob-storage/backups.md` (id `BKUP`) and its neighbours in that directory.
This plan records what that spec asks of bestool, since two of its requirements have no mechanism here today.
The design is not settled; the options below are options, not a decision.

## What the store looks like

A directory tree under a configurable root, keyed by content hash, with a two-level fan-out.
Blobs are immutable once written and are moved into place by an atomic rename, so a reader never sees a partial blob.
Nothing rewrites a stored blob, and only the central server ever removes one (collecting blobs that no record references).

Two consequences for backups.

The store needs no freeze.
A live path capture cannot observe a partial or changing blob, so the `simple` method is sufficient and correct here; a blob admitted mid-capture is simply included or not.
This is worth stating because `simple` otherwise carries the caveat that a live path snapshot has no consistency point, which does not apply to an append-only content-addressed tree.

The store dedupes against itself.
Because content is named by hash and never rewritten, a cycle transfers only blobs added since the previous one, and each capture still represents the whole store rather than a delta needing a chain to restore.

A facility store additionally has two tiers, distinguished only by durability: an outbox holding blobs the central server has not yet acknowledged, which is the only durable copy of that content, and a cache holding blobs that are durable elsewhere.
The tier is recorded in a database table, not in the tree, so it comes back with the database rather than with the store.
Both tiers must be captured. Losing the outbox loses content held nowhere else.

## Requirement 1: ordering within a cycle

The database must be captured first and the store second, so the store capture is a superset of what the database capture references.
Reversed, a blob admitted between the two captures is referenced by the restored database but absent from the restored store.

Today each backup type is independent: `BackupDef` carries `type`, `tags`, `pre`, `post`, and one method, with no way to say "run after that type", and Canopy schedules each `(group, type)` on its own interval and emits due types with no declared order.
The daemon linearises a batch through one run slot, but the order within it is not a guarantee anyone can rely on.

Options:

- Drive the store capture from the database definition's `post` hook, invoking `bestool canopy backup --type <store type>`.
  Ordering falls out for free and needs no new concepts.
  Against it: the hook re-enters the driver and takes a second per-type lock, and the two runs report to Canopy as unrelated, so an operator sees two rows with nothing tying them together.
- Add an ordering or dependency field to the definition, and have Canopy emit a batch in dependency order.
  Cleanest to reason about, and the only option that makes the relationship visible to Canopy, but it touches both sides.
- Make the pair one backup type with two sources.
  Cuts against exactly-one-method-table and against per-type retention and scheduling, so probably not.

## Requirement 2: pairing the two captures

A restore has to select the store capture belonging with the database capture it is restoring.
Today each run mints its own run id and tags its snapshot with device, run, and type, so two captures of the same logical cycle share no identifier, and pairing is left to an operator comparing timestamps by eye.

The freeze moment already reported for a capture is the natural anchor.
The database capture reports the instant it froze; the store capture has no freeze instant of its own, being a live path snapshot, so "the store capture belonging to this database capture" means the earliest store capture taken after that instant.
Either derive the pair that way at restore time, or tag both runs of a cycle with a shared cycle id and pair on it directly.

A partial ordering also helps: a later store capture restored against an earlier database capture is safe, being a superset, whereas an earlier store capture against a later database capture is not.
Restore should be able to fall back to a later store capture, and should refuse an earlier one.

## Requirement 3: restoring the pair

`bestool canopy restore <type> <id>` restores one type from one snapshot named explicitly.
Restoring a server now means restoring two, in the right relationship, which is either a matched pair of invocations or something that understands the cycle.

One part of this is not bestool's: after a restore, Tamanu has to reconcile the tree against its own registry of what it holds, since the two were captured at different moments and can disagree in both directions.
bestool restores files and knows nothing about that table.
Worth being explicit about the boundary so neither side assumes the other does it.

## Retention

Central's store is the authoritative copy of every blob in a deployment and grows without deletion, and Tamanu's integrity spec names a backup as the dependable source for repairing a blob that is corrupt or missing there when no peer holds it.
So the store type's retention wants deciding deliberately rather than inheriting a database-shaped default.
Because each store capture represents the whole store rather than a delta, expiring old captures does not lose a blob that is still on disk, which makes a modest retention safe. A blob deleted from the store and then wanted back is the case retention actually bounds.
