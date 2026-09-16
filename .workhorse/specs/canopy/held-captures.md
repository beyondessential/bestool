---
id: HOLD
---

# Held captures

A backup run's capture — the point-in-time copy of the data that a method prepares for the repository, described in [BAK](backup.md) — can be retained on the device after the run rather than released.
A held capture is a local restore source: restoring from it costs a local copy, where restoring from the repository costs a full download.

The difference matters during an upgrade window.
A capture taken at cutover is the rollback point, and the upload carrying it offsite can run for hours after the data froze; a held capture makes that rollback available for the whole window without waiting for, or depending on, the transfer.
A hold is device-local state, created and released by an operator working on that host.

## Holding a capture

A run retains its capture when asked to at the point it starts, or at any time while it is in flight.

`bestool canopy backup --type <type> --hold` runs an ordinary backup — same preparation, upload, and reporting — and retains the capture at the end instead of releasing it.

`bestool canopy hold keep --type <type>` tells a run already under way to retain the capture it is working from.
The instruction reaches the daemon hosting the run and takes effect when the run finishes; the transfer in progress is not interrupted, slowed, or otherwise altered, so a run that has already spent hours uploading keeps that work.
Only a daemon-hosted run can be reached this way, and the command says so plainly when the named type has no run in flight there.

`bestool canopy hold create <type>` takes a capture and nothing else: no credentials are fetched, no repository is contacted, and no run is reported.
The definition's `pre` and `post` hooks run and the method prepares its capture exactly as it would for an uploading run, so a capture-only hold is the same artefact as a held capture from a full run.
It offers no way to upload, which is why it exists as a command of its own: the same thing is spelled `bestool canopy backup --type <type> --hold --no-upload`, where omitting the second flag starts a transfer that an operator only wanting a rollback point did not ask for and may wait hours to be rid of.

## What a hold consists of

Every method's capture is holdable, whether it is a volume snapshot or a staged base backup.

A hold carries an id, the backup type it came from, the moment the data froze, the path the capture is readable at, whether the run also uploaded it, and whatever the device needs to release the capture later.
This record lives in a fixed device directory — `/var/lib/bestool/held-snapshots/` on Unix, a per-platform data directory on Windows — one file per hold, so a hold survives the daemon restarting and the machine rebooting.

A held capture is exposed at a path of its own, distinct from the path a run exposes its capture at and keyed by the hold rather than by the backup type.
A subsequent run of the same type therefore neither disturbs a hold nor is disturbed by one, and several holds of one type coexist.

## Whether a capture is still there

A hold is a rollback point only for as long as the capture behind it can be read, so each hold is in one of three states.
It is present when the capture reads where the record says it is.
It is detached when the capture is still there but nothing currently exposes it: an exposure is made by the process that made it, and does not always survive that process ending, the machine rebooting, or the platform renumbering the devices it hands out.
It is gone when nothing of the capture is left.

The state is judged by whether the capture reads where a restore would read it, not by whether whatever exposes it appears to be in place.
An exposure can be in place and serve nothing, so judging by the exposure alone reports intact holds as lost, and an operator has no way to tell that apart from a capture that really has gone.

Telling detached from gone is what makes the difference actionable.
A detached capture is recoverable: `bestool canopy hold reattach <id>` exposes it again where its record says it lives.
It reports the capture readable only once it can be read there, so an operator sent to it by another command is never told it worked when nothing changed.
A gone capture is not recoverable, and the hold that names it is no longer a rollback point.

## Releasing a hold

A hold is released only when an operator asks: `bestool canopy hold drop <id>`.
Nothing else releases one — a hold has no expiry, and a later run of the same type leaves it alone.
This is what makes a hold trustworthy as a rollback point for as long as the operator needs it, and it is why forgotten holds are reported (below).

Dropping a hold releases the underlying capture as its method would have at the end of a run, and removes the record.
A hold whose capture has already gone is dropped as far as it can be: the record is removed and the command reports that the capture was already absent, rather than failing.

`bestool canopy hold list` shows the holds on the device: id, backup type, the moment the data froze, how long the hold has been held, whether it was uploaded, and whether its capture is still present.

## Reporting forgotten and lost holds

A doctor check reports the device's held captures, so a hold that outlives its purpose is visible rather than discovered when a volume fills.

The check reports two distinct conditions.
A hold that has been held a long time is untidy and grows more expensive the longer it is kept.
A hold whose capture has vanished is more serious and is reported more severely: the operator believes a rollback point exists when it does not, and the belief is the harm.

Where the platform's snapshot mechanism keeps its copies in a bounded store shared with every other snapshot on the volume, the check also reports that store's headroom, so pressure that would evict a hold is visible before it evicts one.
The device reports this headroom and does not change it: the store is host-wide configuration that backups share with everything else using snapshots on that host, and sizing it is an operator decision.

## Restoring from a held capture

`bestool canopy restore --type <type> --from-hold <id>` restores from a held capture instead of from the repository.
It reads only local data, so it runs without repository access and at local copy speed; the backup type selects the definition and method as it does for a repository restore.

A restore refuses a hold whose capture cannot be read, naming the state the hold is in and what to do about it, rather than laying an empty or partial tree over the data it is replacing.

The capture is copied into the restore staging area, and the method's restore then proceeds exactly as it does for a snapshot fetched from the repository.
Restoring by copy rather than by moving the capture into place is what lets the hold outlive the restore, so a restore that fails partway can be attempted again from the same rollback point.

Staging is a whole second copy of the captured data, so a restore from a hold needs free space for one copy of the capture.
The data it displaces is set aside on the same filesystem rather than copied, and so costs no further room.
The device checks for that space before it begins copying and refuses up front, naming what is needed and what is free, rather than failing partway through a restore an operator is depending on.
Where there is not room, the refusal names restoring in place as the way through, because otherwise a rollback point that exists and is readable is simply unusable.

## Restoring in place

`bestool canopy restore --type <type> --from-hold <id> --in-place` lays a held capture back down over the live data without staging a copy of it first.
It is the only way to roll back on a device whose volume cannot hold two copies of the data, which is the ordinary case for a large cluster on a single-volume host.
Only a held capture is restored this way; a repository restore has already spent the room a staged copy needs by the time it has downloaded the snapshot.

Only what has diverged from the capture is written.
The divergence between a hold and the data it rolls back onto is the writes made since the capture froze — hours, during an upgrade window — rather than the size of the data, so the cost is proportional to that rather than to the capture.
Copying the whole capture back over itself would be far worse than merely slow: where the capture is a snapshot of the same volume, every block written over a block the snapshot still references is copied aside into the copy-on-write store first, and a store that fills is made room in by deleting snapshots — so the source can vanish partway through a copy that has already overwritten the destination.

What diverged is established without reading either side's contents wherever it can be.
An entry present in only one of the two, or present in both at different sizes or as different kinds of thing, is settled from metadata alone.
That leaves entries present in both at the same size, and for those the device asks the filesystem what it changed since the capture, from the record filesystems already keep of their own writes.
Where the filesystem can answer, its answer is authoritative and no contents are read.
Where it cannot — the capture predates the device recording a position to ask about, the record has been discarded, or the backend keeps none — the device falls back to comparing the two sides: an entry whose size and modification time both match is left as it is, and any other is compared by content.
That fallback is weaker, because an entry that diverges without changing either is kept, so a restore says which of the two it used.

Two different resources are consumed, and both are checked before anything is written.
The filesystem holding the data must have room for the net growth: what the new and larger entries add, less what the deleted and smaller ones free.
The copy-on-write store behind the capture must have room for everything written, which is the larger number and, where the capture is a snapshot on the same filesystem, is charged to that same free space.
Where that store is bounded separately — a Windows shadow copy's storage area, a thin pool — its own headroom is checked and a shortfall names how to raise it.
Where the headroom cannot be read the restore proceeds and says so, rather than refusing a rollback on a number it could not obtain.

Restoring in place gives up the atomic swap.
No copy of the displaced data is kept, and while the restore runs the data is neither the state it was in nor the state that was captured.
Data in that condition must not be served, so the restore holds it unstartable for the duration and records in the data itself that it is mid-restore, naming the hold to finish from.
It refuses to start the service while that record is there, and removes it only once the data is the captured state.

The hold is untouched throughout, because reading a capture consumes no copy-on-write space.
An interrupted restore is therefore resumed by running the same command again: laying the divergence down converges, so a second run finishes what the first began rather than starting over.
A restore that finds the data already matching the capture writes nothing and says so.
