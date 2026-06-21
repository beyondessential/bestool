# PostgreSQL physical backup (bestool)

A cross-platform `bestool` command that produces reliable, restorable Kopia
backups of a PostgreSQL cluster's data directory, on Windows and Linux
(btrfs and ext4). It replaces the Ansible `kopia-backup-postgres-*` scripts
(`ops/ansible/roles/postgres/files/`) and the naive "let Kopia VSS-snapshot
the live directory" approach on Windows, behind one tested binary that
integrates with the Canopy backup control plane.

This is the **physical** backup (a restorable file-level image of the
cluster). It is distinct from the existing `bestool tamanu backup`, which is
a **logical** `pg_dump` (a `.dump` file, table-exclusions, encrypt/split) and
stays as-is.

## The core problem: today's backups restore dirty, not clean

Neither current method is bestool — Linux uses the Ansible scripts, Windows
points Kopia at the live directory directly. Both nonetheless end up
restoring through `pg_resetwal -f` + a forced full REINDEX, which is the
direct cause of "partially corrupted, mostly indexes" — but for two
different upstream reasons.

**Linux (Ansible scripts).** They take the btrfs snapshot *between*
`pg_backup_start` and `pg_backup_stop` and write the start-only `backup_label`
returned by `pg_backup_stop` **into** the snapshot. `pg_backup_stop` emits its
`XLOG_BACKUP_END` WAL record *after* the snapshot instant, into the live
`pg_wal`, so the frozen copy lacks it. PostgreSQL requires a
`backup_label`-driven restore to reach the backup-end point — "you cannot use
a base backup to recover to a time when that backup was in progress" — so
recovery fails with **`WAL ends before end of online backup`**. The btrfs
snapshot is itself atomic and would have restored cleanly as crash recovery;
the `backup_label` is precisely what breaks it.

**Windows (Kopia-direct, no bestool today).** No `pg_backup_start` and no
`backup_label` at all — Kopia is pointed at the live data directory with VSS
meant to supply the point-in-time copy. The failure here is a **non-atomic
capture**: if VSS does not actually engage (silent fallback to reading live
files), or `pg_wal`/tablespaces sit on a volume the shadow copy does not
include, Kopia reads files at inconsistent instants. The result is torn
WAL/pages, and recovery hits **`invalid record length`** /
**`could not locate required checkpoint record`**.

**Both land in the same place.** `pgro`'s restore Job
(`pgro/src/controllers/restore/builders.rs`) runs `postgres --single`, greps
for *all* of those signatures (`WAL ends before end of online backup|invalid
record length at|database system was interrupted while in recovery|could not
locate required checkpoint record`), and on any hit runs **`pg_resetwal -f`**,
which *bypasses WAL replay* and forces the cluster up at whatever the snapshot
captured. Skipping recovery leaves in-flight index updates as torn pages
("unexpected zero page at block N"), so the Job `touch`es `needs-reindex-all`
and **REINDEXes every database** before marking the replica ready. It "works
most of the time" only because `pg_resetwal` + REINDEX usually salvages the
cluster; when a torn page lands in the heap or catalog, or discarded WAL
mattered, it doesn't.

**The fix is producer-side and differs by platform.** Linux must stop writing
the `backup_label` (its snapshot is already atomic → clean crash recovery);
Windows must guarantee a genuinely atomic capture (a real VSS set over every
cluster volume) instead of reading the live directory. Both then restore as
clean crash recovery, and `pgro` never needs `pg_resetwal`.

### Field evidence (Windows restores)

Observed symptoms, in increasing severity, all consistent with a non-atomic
capture recovered via `pg_resetwal`:

- B-tree corruption on `logs.changes` (`changes_version_idx`); fixed by
  `REINDEX TABLE logs.changes`.
- B-tree corruption on `notes` (`notes_pkey1`): *"heap tid from index tuple
  points past end of heap page"*; fixed by `REINDEX TABLE notes`. The index
  references heap rows the heap file does not contain — the index was captured
  at a later instant than its heap (temporal skew between two files).
- A migration (`updateEncountersTableSetPatientIdNotNull`) failing with FK
  violation `encounters_patient_id_fkey1`: orphaned `encounters` rows whose
  `patient_id` is absent from `patients`.

The first two are index-vs-heap skew and `REINDEX`-recoverable. The third is
**heap-vs-heap skew** across two tables — impossible in a live cluster (the FK
forbids it), **not** fixable by `REINDEX` (no patient row exists to point at),
and therefore unrecoverable data corruption. It is the clearest proof that the
files in the backup are not from a single instant. Two corollaries:

1. Even a perfectly atomic snapshot would corrupt this way if recovered via
   `pg_resetwal`: heap pages lag the WAL, and `pg_resetwal` discards the WAL
   that crash recovery would have replayed to reconcile them. The fix must
   yield a snapshot whose crash recovery **succeeds**, so `pg_resetwal` is
   never reached.
2. `REINDEX` silently masks the index-level symptoms, so most index damage is
   never noticed; heap-level skew surfaces only when something happens to
   validate it (a constraint, a migration). "Works most of the time" describes
   the *detected* failure rate, not the actual one — restores that appear to
   succeed may carry undetected heap inconsistency.

These are not collation/locale corruption (a known, separate btree-corruption
cause that `pgro` already repairs): the affected indexes are non-text
(`_version_idx`, a `_pkey`), and "heap tid points past end of heap page" is a
structural skew error, not an ordering error.

## Two correct strategies, by filesystem capability

PostgreSQL's file-system-backup docs bless the clean path directly: a frozen
snapshot of a running data directory restores by crash recovery — "it will
think the previous server instance crashed and will replay the WAL log… (and
be sure to include the WAL files in your backup)… You can perform a
`CHECKPOINT` before taking the snapshot to reduce recovery time." No
`pg_backup_start`/`pg_backup_stop`, no `backup_label`. The catch: the
snapshot **must be atomic** across every volume the cluster occupies.

So the strategy splits on whether the platform can take a **cheap** atomic
snapshot. "Cheap" matters: a thick (classic) LVM snapshot *is* atomic, but it
copies-on-first-write for its whole lifetime, degrading live-DB write
throughput (~2–6×) for as long as Kopia takes to read the data dir, and its
fixed CoW area silently invalidates the snapshot if it overflows. That cost is
not worth paying when `pg_basebackup` is available, so thick LVM is treated as
"no usable snapshot" and routed to Strategy B.

| Platform / FS | Cheap atomic snapshot? | Strategy |
|---|---|---|
| Windows | Yes — VSS shadow-copy set | `s[snap.*]` crash-consistent snapshot |
| Linux btrfs | Yes — subvolume snapshot | `s[snap.*]` crash-consistent snapshot |
| Linux thin LVM | Yes — thin snapshot (pool CoW, no read-copy) | `s[snap.*]` crash-consistent snapshot |
| Linux thick LVM | Atomic but **costly** (write amplification) | `s[base.*]` `pg_basebackup` |
| Linux bare ext4/xfs (no LVM) | No | `s[base.*]` `pg_basebackup` |

For the snapshot-capable cases the design is uniform; only the snapshot
primitive differs (VSS / btrfs / thin LVM). Everything else — a thick LV, or a
plain partition with no volume manager — has no *cheap* atomic-snapshot
primitive and takes the WAL-complete `pg_basebackup` path.

### Strategy A — crash-consistent snapshot (Windows VSS, btrfs, thin LVM)

s[snap.checkpoint]
Open a libpq connection and issue an explicit `CHECKPOINT` immediately before
snapshotting, to bound WAL replay on restore. This is an optimisation, not a
correctness requirement.

s[snap.no-backup-api]
Do **not** call `pg_backup_start`/`pg_backup_stop` and do **not** write a
`backup_label`. The atomic snapshot is self-contained and restores as crash
recovery. This is the single most important change from the current scripts:
writing a start-only `backup_label` is what forces `pg_resetwal` downstream.

s[snap.atomic-set]
Take a **single atomic snapshot covering every volume** the cluster occupies
(see `s[common.volumes]`). On Windows this is one VSS snapshot **set** (VSS
supports atomic multi-volume sets); on btrfs, snapshot every involved
subvolume. Thin LVM cannot atomically snapshot multiple LVs in one operation,
and btrfs multi-subvolume snapshots are likewise not a single atomic act, so
when the cluster spans more than one volume/subvolume on those backends, route
to Strategy B (`pg_basebackup`) rather than take non-simultaneous snapshots.
If volumes that must be captured together cannot be, abort loudly
(`s[goal.loud-failure]`) — a non-simultaneous multi-volume snapshot is exactly
the corruption case PostgreSQL warns about. (The common single-volume Tamanu
layout — data and `pg_wal` on one filesystem — is the easy path.)

s[snap.include-wal]
`pg_wal` **must** be inside the snapshot set. Never exclude it, never ignore
it in Kopia (`s[common.ignore]`). Without it there is no WAL to replay and the
restore cannot reach consistency.

s[snap.frozen-source]
Expose the snapshot read-only and point Kopia at the data directory **within
the frozen snapshot**, never at the live directory. (Windows shadow copies
are read-only; that is fine here because no `backup_label` needs writing into
them.)

### Strategy B — `pg_basebackup` base backup (thick LVM, or no volume manager)

s[base.rationale]
When there is no cheap atomic snapshot — a plain partition, or a thick LV
whose snapshot would be too costly — a file-level copy of the live directory
is inherently torn and needs WAL spanning the copy to become consistent.
`pg_basebackup --wal-method=stream` produces a correct base backup with that
WAL **and the backup-end record** bundled in — so it restores as clean crash
recovery, with no `pg_resetwal` and no forced REINDEX. This is the right tool
when Strategy A is unavailable.

s[base.flow]
Run `pg_basebackup -D <staging> --wal-method=stream --checkpoint=fast` into a
staging directory, then Kopia-snapshot the staging directory. Restore is
crash recovery from the bundled WAL.

s[base.cost]
This streams a full copy each run (Kopia still dedupes the result in the
repo, but local staging needs roughly the cluster size in free space and a
full read each run). It is strictly heavier than a CoW snapshot. The preferred
long-term fix is to move these hosts onto btrfs and use `s[snap.*]`;
`pg_basebackup` is the correct path until then. We control no thick-LVM/bare
hosts today — the controlled fleet is btrfs — so Strategy B exists for
**externally-provisioned** hosts (typically Ubuntu Server's default install,
which is thick LVM, not thin).

## Common behaviour (all strategies)

s[common.volumes]
Resolve PGDATA via the Tamanu config and enumerate **every** volume the
cluster occupies: the data directory, `pg_wal` (follow it if it is a
junction/symlink to another volume — Windows installs are known to relocate
`pg_wal`, per the empty-`pg_wal` handling in `pgro`'s restore script), and
every tablespace under `pg_tblspc`. Strategy A must snapshot all of them
together; Strategy B's `pg_basebackup` handles this inherently.

s[common.tablespaces]
Tablespaces are absolute symlinks under `pg_tblspc` pointing at the live
volume. For Strategy A, record the tablespace OID → snapshot-volume mapping
so restore can relink correctly; for the common single-tablespace Tamanu
install `pg_tblspc` is empty and this is a no-op.

s[common.stable-source]
Kopia must see a **stable** source path across runs (a fixed mount/junction),
so snapshot history, incremental dedup, and retention attribute to one
source. A per-run path makes every backup a fresh single-snapshot source with
no history — the exact regression just fixed in the Ansible btrfs script.

s[common.ignore]
Ignore only transient files: `postmaster.pid`, `*.log`, `pg_stat_tmp/*`,
`lost+found`. **Never** ignore `pg_wal`, `pg_xact`, `pg_control`, `global`, or
any tablespace.

s[common.metadata]
Record backup metadata — PG major version, control-file checkpoint LSN,
snapshot timestamp, strategy used, volume/tablespace mapping, cluster size —
as Kopia snapshot tags/description (and/or a sidecar object). This drives
observability and lets the external verifier drive a restore. It does **not**
need to be written into a read-only snapshot.

## Canopy integration

This command is the device-side **producer** for the Canopy backup control
plane (see `pgro/docs/canopy-backup-integration.md` and the canopy repo's
`docs/plans/backup-credentials.md`). Canopy tracks three signals: *backed up*
(1), *persisted* (2), *restorable* (3); this command produces 1 and 2, `pgro`
produces 3.

s[canopy.type]
Snapshots are the **`tamanu-postgres`** type — the same type `pgro` restores
and reports on. Tag snapshots with the canopy type so `pgro` can filter the
repo to its snapshots.

s[canopy.source-model]
Use Canopy's source/attribution model — the `canopy@<server-id>:<path>` Kopia
source identity and `canopy-run` tagging — so snapshots join against canopy's
`backup_repo_snapshots` / `backup_runs` and `pgro`'s `snapshot_id`
cross-reference. (`s[common.stable-source]` is what makes this attribution
stable.)

s[canopy.credentials]
Obtain the Kopia repository connection (short-lived, per-group S3 credentials
+ target + repo password) from Canopy via the device credential-process flow,
rather than long-lived static keys. This reuses bestool's existing
`canopy`/`kopia` plumbing.

s[canopy.report]
Report backup outcome (signals 1/2) to Canopy on completion, consistent with
the control-plane contract. Restorability (signal 3) is `pgro`'s job, not
this command's.

## Restore

s[restore.crash-recovery]
Restore for all strategies is plain crash recovery: lay the snapshot/base
backup down as the data directory, start PostgreSQL, let it replay `pg_wal`.
No `recovery.signal`, no `restore_command`, no `backup_label`.

s[restore.consumer]
`pgro` is the restore/verification consumer. Once this command ships clean
crash-consistent backups, `pgro`'s `pg_resetwal -f` + forced-REINDEX path
(`builders.rs`) should stop triggering for `tamanu-postgres` snapshots and
become a true last-resort fallback. That change is tracked on the `pgro` side;
this spec's job is to stop *producing* the dirty snapshots that force it.

## Reliability and lifecycle

s[life.loud-failure]
Any inability to obtain an atomic snapshot (VSS failure, missing elevation,
a multi-volume layout that cannot be captured together) must fail the backup
non-zero and never silently fall back to copying the live directory — silent
live-file fallback is the likely current Windows corruption cause.

s[life.reaper]
On startup, before creating a new snapshot, sweep leftovers from a previously
crashed run — VSS shadow copies / btrfs `@…-kopia-*` subvolumes / LVM
snapshots and their exposed mounts. A hard reboot mid-run skips cleanup, and
leaked snapshots fill the volume. (The Ansible btrfs script gained this; the
command must have it on every platform.)

s[life.cleanup]
On success or failure, release the snapshot/shadow-copy set, remove exposed
mounts/junctions, and (Strategy B) delete the staging directory.

s[life.single-run]
Prevent overlapping runs with a lock; a run that overruns its schedule must
not have another start on top of it.

s[life.elevation]
On Windows the command needs Administrator rights for VSS; on Linux it needs
the privileges for the snapshot primitive (root/CAP_SYS_ADMIN for btrfs/LVM).
Detect insufficient privilege and fail clearly per `s[life.loud-failure]`.

s[life.scheduling]
Runs on a schedule (Windows Task Scheduler / systemd timer) at low I/O and
CPU priority, at a configurable interval. 6-hour RPO (the current Linux timer)
is acceptable; there is no point-in-time recovery and each backup is
self-contained.

## Command surface

s[cmd.placement]
A new subcommand alongside the logical `bestool tamanu backup` — e.g.
`bestool tamanu physical-backup` (final name TBD; it must not collide with or
change the existing logical `backup`). It discovers PGDATA and the DB
connection from the Tamanu config like the existing command does.

s[cmd.strategy-detect]
Detect the storage backend and pick the strategy automatically, with an
override flag for testing. The decision walks from the data directory down to
the backing block device:

1. **Windows** → Strategy A (VSS).
2. Filesystem is **btrfs** (`findmnt -no FSTYPE` / `statfs`) → Strategy A
   (subvolume snapshot).
3. Backing device is an **LVM thin LV** → Strategy A (thin snapshot). Resolve
   the mount's source device, then check its LVM segment type — e.g.
   `lvs --noheadings -o segtype <dev>` reports `thin` for a thin LV (and it
   has a `pool_lv`), vs `linear`/`striped` for a thick LV. Only `thin`
   qualifies.
4. **Anything else** — thick LV, or a plain partition with no LVM at all →
   Strategy B (`pg_basebackup`).

This is a strict superset of the Ansible `backup.yml` btrfs-vs-ext4 detection;
the new work is distinguishing thin from thick LVM (step 3). If detection is
ambiguous, prefer Strategy B — it is always correct, just heavier.

## Verification

Backup verification is **out of scope** for this command — it is done
off-host by `pgro` (signal 3), exercising the whole pipeline. This command's
job is to make those verifications pass cleanly. It emits restore metadata
(`s[common.metadata]`) so the verifier can drive a restore unambiguously.

## Goals

s[goal.clean-restore]
Backups must restore as clean crash recovery, without `pg_resetwal` and
without a forced full REINDEX.

s[goal.atomic]
Strategy A backups come from an atomic, point-in-time snapshot covering every
cluster volume; never from the live data directory.

s[goal.self-contained]
Each backup is self-contained — restorable on its own, with all needed WAL
included — matching the current no-PITR / RPO-equals-interval model.

s[goal.loud-failure]
Never silently degrade to an unsafe copy; fail loudly instead.

s[goal.canopy]
Integrate with Canopy as the `tamanu-postgres` producer (signals 1/2), using
short-lived per-group credentials.

## Open questions

- **Command name/placement** under `bestool tamanu` (`physical-backup` vs
  `snapshot` vs a `--physical` mode), and whether it shares code with the
  `kopia` action group.
- **Windows volume layout in practice**: how often is `pg_wal` relocated to a
  separate volume, and are tablespaces ever used? Determines how much
  multi-volume VSS-set handling is exercised vs. defensive. (`pgro` already
  handles a missing `pg_wal` on restore, implying it does happen.)
- **Strategy B fleet**: the controlled fleet is already btrfs; Strategy B
  exists only for externally-provisioned hosts whose storage we don't dictate
  (assumed Ubuntu-default thick LVM). Confirm the thin-vs-thick LVM detection
  (`s[cmd.strategy-detect]` step 3) against a real such host, since that's the
  one new branch and the assumption ("probably thick") should be verified, not
  trusted.
- **Credential lifetime vs. backup duration**: Canopy's short-lived creds —
  does a long `pg_basebackup`/snapshot upload outlive them? (`pgro` flags the
  1-hour chained-AssumeRole cap as its hardest problem for long restores; the
  producer side should check the same for long uploads.)
- **Coexistence with the Ansible scripts** during rollout: the command and
  the scripts must not both drive backups on the same host.
