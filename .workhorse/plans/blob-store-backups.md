# Plan: blob store backups

Tamanu is moving attachment and asset bytes out of the database into a
content-addressed blob store on disk.
Once it does, a `tamanu-postgres` backup no longer captures a server's whole state: the database holds references and the store holds the bytes, and the two have to be backed up and restored consistently.

The behaviour is specified on the Tamanu side, in `specs/blob-storage/backups.md` (id `BKUP`) and its neighbours in that directory.
This plan records what that spec asks of bestool, since its requirements had no mechanism here.
Each requirement below now carries its decision; the rejected options are kept for the record.

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

**Decided: a follower field on the definition, chained by the driver.**
A def may declare `after = "<type>"`.
When a run of that type completes a real backup successfully, the driver then runs every def that names it, sequentially, before returning; the chain carries a visited set so a definition cycle cannot loop.
A run that was skipped (lock already held) or exited dormant chains nothing.
The store def is the one carrying `after = "tamanu-postgres"`, so deployments add one file and the existing database definition and schedule are untouched.

This needs nothing from Canopy.
The follower type registers as a capability like any other, and a Canopy schedule on it is optional and safe: a store capture taken on its own is a superset of every earlier database capture, so an independently scheduled run is a valid backstop for a cycle whose chain did not fire (say, database backups failing for a week while outbox content accrues).
In the daemon, the chained run happens inside the leader's runner, so a cycle occupies the daemon's single run slot end to end and no other backup interleaves between the two captures.
The two runs still report to Canopy separately; what ties them together is the pairing rule below, not a shared identifier.

Rejected options:

- Drive the store capture from the database definition's `post` hook, invoking `bestool canopy backup --type <store type>`.
  Ordering falls out for free and needs no new concepts.
  Against it: post-hook failures are deliberately swallowed, so a failed store capture would not fail anything; the leader's Canopy report is delayed behind the whole store upload; and under the daemon the re-entrant invocation waits on the run slot the leader itself holds, which deadlocks unless the hook bypasses the daemon.
- Add an ordering or dependency field to the definition, and have Canopy emit a batch in dependency order.
  Touches both sides, and still does not tie the pair together when the two types' schedules drift apart, which is the normal case.
- Make the pair one backup type with two sources.
  Cuts against exactly-one-method-table and against per-type retention and scheduling, so no.

## Requirement 2: pairing the two captures

A restore has to select the store capture belonging with the database capture it is restoring.
Today each run mints its own run id and tags its snapshot with device, run, and type, so two captures of the same logical cycle share no identifier, and pairing is left to an operator comparing timestamps by eye.

The freeze moment already reported for a capture is the natural anchor.
The database capture reports the instant it froze; the store capture has no freeze instant of its own, being a live path snapshot, so "the store capture belonging to this database capture" means the earliest store capture taken after that instant.

A partial ordering also helps: a later store capture restored against an earlier database capture is safe, being a superset, whereas an earlier store capture against a later database capture is not.
Restore should be able to fall back to a later store capture, and should refuse an earlier one.

**Decided: derive the pair at restore time from the repository's own snapshot times.**
Restoring a type also restores each def that declares `after` on it, selecting the earliest snapshot of the follower's type from the same source host whose start time is at or after the chosen snapshot's start time.
kopia's snapshot start time is a conservative stand-in for the freeze moment: the freeze is never later than the moment kopia began uploading, so any store capture that began at or after the database capture's start began after the freeze, and the comparison can only refuse a capture that was actually safe, never accept one that was not.
Where no follower snapshot at or after exists, the restore refuses up front, before touching any data; an earlier store capture is never selected.
`--no-followers` restores the named type alone, and an explicit `restore <store type> <id>` remains the operator's manual path, so the refusal has a deliberate way past it.

To make the follower's snapshots identifiable in the repository, every snapshot now carries its backup type as the kopia description.
Snapshot tags do not round-trip through `kopia snapshot list`, which is also why a shared cycle id tag was rejected; the other route to a cycle id, a new field on the run report, is a Canopy change this card deliberately avoids.
As a fallback for snapshots without a description, a source path ending in `backup-source/<type>` (the stable per-type view path on Linux) classifies the same way.

## Requirement 3: restoring the pair

`bestool canopy restore <type> <id>` restores one type from one snapshot named explicitly.
Restoring a server now means restoring two, in the right relationship, which is either a matched pair of invocations or something that understands the cycle.

**Decided: the restore command understands the cycle.**
`restore tamanu-postgres <id>` plans the whole cycle first, selecting the paired follower snapshots by the rule above and failing before anything is overwritten if one cannot be paired, then restores the database and then the store.
Each half is a normal single-type restore with its own credentials, run id, and Canopy report.
The database goes first so that, on a replacement host, the store's own restore target can be resolved from the freshly restored database (see the store root below).

One part of this is not bestool's: after a restore, Tamanu has to reconcile the tree against its own registry of what it holds, since the two were captured at different moments and can disagree in both directions.
bestool restores files and knows nothing about that table.
Worth being explicit about the boundary so neither side assumes the other does it.

## Requirement 4: where the store root comes from

The store root is a Tamanu setting, not config: `blobStorage.root`, default `data/blobs`, resolved against the server's working directory when not absolute, editable in the admin panel, applying on restart.
No config file carries it, so `bestool tamanu config` cannot report it, and a definition that hardcodes the path can be silently orphaned by an administrator moving the store.

**Decided: the definition resolves the path at run time instead of hardcoding it.**
The `simple` method accepts `path_command` as an alternative to `path` (exactly one of the two): an argv-style command whose output is the absolute path to capture or restore.
A new `bestool tamanu blob-root` prints the resolved store root: it reads the Tamanu config for database credentials, queries the `settings` table for `blobStorage.root` at the server's scope, falls back to the schema default, and resolves a relative value against the server package directory, which is the server's working directory under pm2.
Resolution runs fresh on every backup, so an admin-panel change is picked up on the next cycle rather than silently diverging, and a restore resolves the store target from the database it has just restored, which is exactly where the restored server will look.
This also means Tamanu does not need to expose the root through config for bestool's sake.

## Retention

Central's store is the authoritative copy of every blob in a deployment and grows without deletion, and Tamanu's integrity spec names a backup as the dependable source for repairing a blob that is corrupt or missing there when no peer holds it.
So the store type's retention wants deciding deliberately rather than inheriting a database-shaped default.
Because each store capture represents the whole store rather than a delta, expiring old captures does not lose a blob that is still on disk, which makes a modest retention safe. A blob deleted from the store and then wanted back is the case retention actually bounds.

**Decided: retention is Canopy configuration, not code, and the guidance is recorded here.**
Pairing never needs depth: a later store capture pairs safely with every earlier database capture, so the latest store capture alone satisfies every retained database capture.
Depth on the store type serves only the repair case, a blob deleted from the store and later wanted back, which on central means the deployment's archive.
So central's store type wants retention at least as deep as its database type, while a facility's store type can be modest, since facility content is durable on central once acknowledged and only the outbox window is at stake.
