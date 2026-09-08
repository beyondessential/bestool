# Audit database redesign for a write-mostly workload

Working document for card C2.
This is a problem-space exploration, deliberately wide.
Nothing here is decided.

## The problem in one paragraph

bestool-psql records every executed statement into a local audit database.
The store is redb, which permits one process per file.
Several interactive sessions commonly run at once for the same OS user, so each session copies the whole main file, writes to its copy, syncs deltas back on a timer and at exit, and a startup task hunts for copies left behind by crashed or failed sessions and merges them back.
The copy, the sync, the orphan hunt and the merge are all workarounds for the one-writer limit, and each has its own failure modes, including out-of-memory during orphan recovery.

## How it works today

Code lives in `crates/psql/src/audit/`.

- `database.rs` opens the store.
  It creates `audit-main.redb` if missing, copies it to `audit-working-<uuid>.redb` with reflink where the filesystem allows, opens the copy read-write, starts a sync thread, and starts an orphan-recovery thread.
- `multi_process.rs` holds the sync and recovery logic.
  Sync runs every 60 seconds and at shutdown: it scans the whole working history table, collects entries newer than the last synced key, opens main read-write with jittered retries, and inserts them.
  Recovery looks for `audit-orphaned-*` files and for `audit-working-*` files whose mtime is older than 60 seconds, tries an exclusive open to confirm nobody holds them, loads every entry of the file into a vector, and inserts all of them into main in one transaction.
- `entry.rs` defines the record: query, db user, OS user, write-mode flag, OTS supervisor, the active untagged Tailscale peers (hostname and login name each, typically two or three), instance uuid, recall flag.
  Records are JSON strings keyed by a microsecond Unix timestamp.
- `index.rs` keeps a second table mapping history position to timestamp so rustyline can address history by index.
- `history.rs` implements rustyline's history trait on top of the store: each arrow press or search step does index lookup then entry lookup, two read transactions per entry.
- `library.rs` implements the export used by `bestool-psql-audit` and `bestool audit-psql`: it loads every entry, filters by since and until, then applies a limit from either end.
  A flag reads orphan files instead of main.

Shipped targets include Linux musl and gnu, Windows msvc, and macOS, so any replacement must behave on NTFS and with Windows file-locking semantics.

## Defects observed while reading

These are facts about the current code, listed because they shape what a replacement must fix and what it must not regress.

- **Culling never runs in production.**
  The size cap of 100 MB is enforced only inside the branch that creates a brand-new main database.
  Once main exists it grows without bound.
  Compaction is also a no-op in multi-process mode.
- **Orphan merge is the memory blow-up.**
  An orphan is a full copy of main plus that session's entries, and the merge loads all of it into memory and reinserts everything into main in a single write transaction.
  With several orphans, or a large main, this is many multiples of the main file held in memory at once.
- **Every session costs a full copy of main on disk** for the life of the session, unless the filesystem supports reflink (btrfs, XFS with reflink, ReFS; not ext4, not NTFS).
- **Periodic sync scans the whole working table** rather than a range from the last synced key.
- **Microsecond timestamps are the primary key**, so two sessions recording in the same microsecond silently overwrite each other on merge.
  Unlikely per pair, not impossible across many sessions and years.
- **Index culling is quadratic.**
  Removing a prefix rewrites the entire index table per batch of 100.
  Moot while culling does not run, but it would bite the moment it did.
- **Orphan detection is mtime-based.**
  A live session that has been idle for a minute looks like an orphan and is saved only by the exclusive-open check, which depends on redb's lock behaving identically on every platform.
- **Audit write failures are logged at debug level and swallowed** at the call sites in the REPL.
  A session can run with no audit trail and never tell the user.

## Workload characterisation

**Writers.**
One record per executed statement, at human pace.
Several sessions per OS user at once, single digits.
Different OS users write to different state directories, so cross-user concurrency does not arise.
Writes must not perceptibly stall the prompt.

**Readers.**

1. Session-local history: up and down through recent entries, reverse search.
   Needs ordered access to the recent tail, at most a few thousand entries, filtered by the recall flag.
   Cross-session live visibility is not a current property: a session sees other sessions' entries only from the next launch.
2. Audit export: by time range, with a limit from either end, as JSON lines.
   Rare, offline in spirit, may run while sessions are live.

**Durability and retention.**
An audit record must not be lost once the statement ran.
Retention today is intended to be a size cap with oldest-first culling, which suits history and is questionable for an audit log.
Nothing today makes the log tamper-evident.

**Size.**
Record size is dominated by the query text; the Tailscale field is a handful of short hostname and login pairs, and the rest is fixed-size metadata.
A single user's store on a developer machine is under a megabyte; server stores shared by an ops team will be larger and are the ones that matter.

## Two concerns hiding in one store

The current design serves two workloads with one B-tree, and they pull in opposite directions.

| | Audit log | Shell history |
|---|---|---|
| Access | append, then rarely read | read the recent tail constantly |
| Ordering | by time, across sessions | by position, within what this session knows |
| Loss | unacceptable | tolerable |
| Retention | long, ideally by age | short, by count |
| Writers | many processes | one process |
| Nice to have | tamper evidence, shippable off-box | fast, in memory |

Separating them is the single move that opens up the design space.
The audit store then only has to do two things well: append from many processes without coordination, and read a tail or a time range.
History becomes a derived view loaded at startup.

## Technique sweep

Grouped by family.
Each entry notes what it buys and what it costs against the workload above.

### Append-only log files

1. **One shared file, appended with O_APPEND (FILE_APPEND_DATA on Windows).**
   The kernel serialises the seek-and-write of each append, so records from different processes do not interleave as long as each record is one write call.
   Framing is either length-prefix plus checksum, or JSON lines with a checksum field; readers skip a torn tail.
   Buys zero coordination, zero copies, crash safety by construction, and a file you can tail or grep.
   Costs: reads are scans (fine, they are rare); retention needs rotation; the non-interleaving guarantee is practical rather than formal for records larger than a page, which a long pasted query or a `\e` buffer can exceed; unreliable on network filesystems.
2. **One file per session.**
   `audit-<uuid>.log`, written only by its own process.
   The directory is the database; readers merge segments by time.
   Buys everything in 1 and removes the interleaving question entirely, because there is never a second writer to any file.
   The orphan concept disappears: a segment left by a crash is simply complete up to its last whole record, nothing to recover.
   Costs: one file per launch accumulates; needs compaction or per-period grouping.
3. **One file per period (day or hour), appended with O_APPEND, per-session fallback if append is refused.**
   Bounded file count, retention by deleting old files, time-range queries open only the relevant files.
   A hybrid of 1 and 2.
4. **Hash-chained records** for tamper evidence.
   Each record carries the hash of the previous record in its segment; optionally sign closed segments with a per-device key.
   Layers cheaply onto any of 1 to 3.
   Worth reserving a field for even if not built now.

### Embedded databases that permit several processes

5. **SQLite in WAL mode via rusqlite (bundled).**
   Multi-process via file locks, one writer at a time with a busy timeout, readers unblocked.
   The "few writers" caveat is about throughput contention, which does not arise at human typing rates with single-digit sessions.
   Buys indexed time-range queries, limits, ordering, and a mature story on NTFS, ext4, APFS.
   Costs: a C dependency (bundled SQLite compiles for musl, msvc and darwin, but the psql crate currently looks pure-Rust by choice: `aegis` with the pure-rust feature, Turso rather than rusqlite); `-wal` and `-shm` sidecar files; known trouble on network filesystems.
6. **Turso.**
   Already a dependency, used for exporting query results to SQLite files.
   Pure Rust.
   Its compatibility notes only say mixed SQLite and Turso multi-process is unsupported and say nothing about Turso-only multi-process access; MVCC is marked experimental.
   Treat multi-process safety as unverified until checked against the Turso source or maintainers.
7. **DuckDB.**
   Columnar, analytics-shaped, one writer or many readers but not both, large binary.
   Poor fit, as the card description already notes.
8. **LMDB via heed.**
   Multi-process readers plus one writer under a file lock, mmap-based.
   C dependency, fixed map size, mmap quirks on Windows.
   redb is the pure-Rust LMDB-alike, and it dropped exactly the multi-process part.
9. **fjall, sled, RocksDB.**
   Single-process LSM stores.
   Same wall as redb.
10. **Keep redb, add a cross-process advisory lock, and open-write-close around every append.**
    Removes copies, sync, orphans and recovery outright.
    Costs: redb open runs repair after an unclean close, which could be slow on a large file and happens at the worst moment; history reads either repeat the open dance per keystroke or move to an in-memory view.
11. **Postgres itself.**
    We are a Postgres client; an audit table in the target database is multi-writer and centralised.
    Rejected as the primary store: the log must exist when the connection fails, in read-only sessions, and for users without write grants, and writing the audit into the audited system lets the auditee edit the record.
    Plausible as a secondary sink.

### OS coordination primitives

12. **Advisory file locks (flock, LockFileEx) around short critical sections.**
    Cross-platform via a small crate.
    A building block for 1, 3 and 10 rather than a design on its own.
13. **A local daemon owning the store**, sessions forwarding records over a Unix socket or named pipe, spawned on demand with lock-based election.
    The ssh-agent shape.
    Costs: lifecycle management on three platforms, a fallback path when the daemon is absent, and far more machinery than the problem warrants.
14. **Leader election among live sessions**, first session writes and others forward to it.
    Worse failure modes than 13 when the leader exits first.

### Merge-friendly keys and compaction

15. **Key records by (session uuid, sequence number) rather than timestamp.**
    Keys can never collide, and any union of records is a valid merge, so concatenating segments is the merge.
    Orthogonal to the storage choice and fixes the timestamp-collision defect regardless.
16. **Periodic compaction.**
    When segment count or total size crosses a threshold, merge closed segments into one time-ordered file under a lock, applying retention at the same time.
    Turns the many-small-files cost of 2 into a bounded, occasional job that can run in the export tool or at startup.

### Off-box and OS-native sinks

17. **Ship closed segments to a central sink** (object storage, canopy, syslog).
    An audit log that lives only on the box it describes is a weak audit log.
    Out of scope for this card, but the local format should make "everything since cursor X" cheap, and append-only segments make it trivial.
18. **Use journald or the Windows Event Log as the store.**
    They are append-only, multi-writer, with retention and forwarding built in.
    Not portable (no journald in static musl builds or on macOS), and history would still need its own store.

### History as a derived view

19. **Load the recent tail into memory at startup**, then append in memory as the session runs.
    rustyline's own memory-backed history can hold it, seeded from the audit store's last N recall-eligible records.
    Removes all read pressure from the audit store, which then needs only append and read-tail.
    No regression, since live cross-session visibility does not exist today.
20. **A small per-user history cache file** rebuilt from the audit store when missing.
    Only worth it if startup tail reads turn out slow, which is unlikely for a few thousand records.

## Constraints to settle before narrowing

Open questions, in rough order of how much each one prunes the shelf.

1. Is a C dependency acceptable in bestool-psql, or is pure Rust a requirement?
   This alone decides between the SQLite family and the log-file family.
2. Should an audit log ever cull itself by size, or is retention by age (or never, with off-box shipping) the right model?
3. Should a failed audit write be loud: warn the user, or refuse to run the statement?
4. Is tamper evidence wanted now, later, or never?
5. Do we need to run on network or synced filesystems (roaming profiles, NFS home directories)?
   Both O_APPEND and SQLite WAL degrade there.
6. Is live cross-session history visibility wanted, or is startup-time visibility enough as today?
7. Does the export tool's interface need to stay identical, including the `--orphans` flag, which stops meaning anything under most options?
8. Migration: existing `audit-main.redb` and any orphan files need a one-shot import into whatever replaces them.

## Early leaning

Not a decision, recorded so it can be argued with.

Per-session append-only segments (2) with records keyed by session uuid and sequence (15), a tail read into an in-memory history at startup (19), export as a time-ordered merge over segments, and compaction with retention (16) when the directory grows.
It matches the workload exactly, has no coordination at all, every failure mode is benign, migration is a one-shot export of the redb file into a segment, and the format is greppable and shippable.
The serious alternative is SQLite in WAL mode (5), which fits comfortably at these rates and buys indexes and a query language at the cost of a C dependency and sidecar files.
Question 1 above decides between them.
