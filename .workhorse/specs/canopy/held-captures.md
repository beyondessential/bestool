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

`bestool canopy backup --type <type> --hold --no-upload` takes a capture and nothing else: no credentials are fetched, no repository is contacted, and no run is reported.
The definition's `pre` and `post` hooks run and the method prepares its capture exactly as it would for an uploading run, so a capture-only hold is the same artefact as a held capture from a full run.

## What a hold consists of

Every method's capture is holdable, whether it is a volume snapshot or a staged base backup.

A hold carries an id, the backup type it came from, the moment the data froze, the path the capture is readable at, whether the run also uploaded it, and whatever the device needs to release the capture later.
This record lives in a fixed device directory — `/var/lib/bestool/held-snapshots/` on Unix, a per-platform data directory on Windows — one file per hold, so a hold survives the daemon restarting and the machine rebooting.

A held capture is exposed at a path of its own, distinct from the path a run exposes its capture at and keyed by the hold rather than by the backup type.
A subsequent run of the same type therefore neither disturbs a hold nor is disturbed by one, and several holds of one type coexist.

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

The capture is copied into the restore staging area, and the method's restore then proceeds exactly as it does for a snapshot fetched from the repository.
Restoring by copy rather than by moving the capture into place is what lets the hold outlive the restore, so a restore that fails partway can be attempted again from the same rollback point.

Staging is a whole second copy of the captured data, so a restore from a hold needs free space for one copy of the capture.
The data it displaces is set aside on the same filesystem rather than copied, and so costs no further room.
The device checks for that space before it begins copying and refuses up front, naming what is needed and what is free, rather than failing partway through a restore an operator is depending on.
