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
  The size cap of 100 MB is enforced only inside the branch that creates a brand-new main database, so once main exists it grows without bound, and compaction is a no-op in multi-process mode.
  This may well be the intended outcome (an audit log should not cull itself by size), in which case the dead code is the defect, not the behaviour.
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
   Costs: a C dependency; `-wal` and `-shm` sidecar files; known trouble on network filesystems; and it would put a second SQLite implementation in the same process as Turso, which is ruled out below.
6. **Turso.**
   Already a dependency, used for exporting query results to SQLite files.
   Pure Rust.
   Cross-process access exists behind an option named `experimental_multiprocess_wal`.
   As of mid-2026 a WAL lock failure on macOS is an open bug and a Windows lifetime-lock bug was fixed in July.
   Not a foundation for a must-never-lose log on three platforms today; worth re-evaluating when the option loses its experimental label.
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

### Compression

Two distinct angles: shrinking the encoding by design, and running a codec over it.
A rough size estimate frames how much either matters: a record is the query text plus about 150 bytes of JSON metadata, and a heavy operator issuing a few hundred statements a day produces on the order of tens of megabytes a year.
Storage volume is not the driver; compression earns its place only if retention becomes long or the log gets shipped off-box.

21. **Context records instead of per-record metadata.**
    Everything except the query and timestamp is session state that changes rarely: OS user, database user, write mode, OTS supervisor, Tailscale peers, instance uuid.
    Write a context record when any of it changes and let query records carry only sequence, timestamp and text; readers carry the context forward.
    This is compression by schema, needs no codec, cuts the per-record write to roughly the query text, and reads naturally as an event log.
    Fits per-session segments exactly, since a segment starts with its context and a torn tail loses at most the last query, never the context.
    Costs: export has to reconstruct flat entries, and a reader that starts mid-segment must first scan back to a context record.
22. **Per-record codec compression.**
    Records are too small for a general compressor to gain anything on its own; a trained dictionary fixes that, since the fixed vocabulary (JSON keys, SQL keywords, table names) lands in the dictionary.
    Dictionary versioning becomes part of the format: each record names its dictionary, dictionaries are shipped with the binary or stored beside the segments, and an old dictionary must stay readable forever.
    Zstd is the natural codec.
    The `zstd` crate wraps C but is already in bestool's dependency tree through the self-update downloader, so it adds no new platform cost to the bestool binary, only to a standalone bestool-psql build.
    `ruzstd` is pure Rust with a working encoder, though its dictionary support for encoding reads as unfinished and its ratio and speed lag the original.
23. **Streaming compression of a whole segment.**
    A single compressor stream per segment shares context across every record, so repetitive queries and metadata compress well without a dictionary.
    Flush after each record so a crash loses nothing already flushed; a truncated frame decodes cleanly up to its last complete block.
    Fits per-session segments (one writer, one stream, never reopened for writing) and is incompatible with a shared O_APPEND file, where interleaved streams from several processes would be unreadable.
    Costs the ability to grep or tail the live file, and framing shifts from newline-delimited to length-prefixed.
    Pure-Rust options include `lz4_flex` frames (fast, modest ratio) and `ruzstd`; a compact JSON-lines file usually compresses several-fold under either.
24. **Compress only at compaction.**
    Live segments stay plain so they can be inspected and are trivially crash-safe; compaction rewrites closed segments into compressed, time-ordered files and applies retention.
    This is the tiered-storage shape and it keeps the codec choice out of the hot write path entirely, so it can be revisited later without a format migration for live segments.
25. **Interning query text.**
    Repeated queries (`\d` on the same table, the same health-check select) could be stored once and referenced by hash.
    Within a segment a streaming codec already captures this; across segments it needs a shared dictionary file and therefore coordination.
    Not worth building on its own.

Compression interacts with tamper evidence only in that the hash chain must be defined over one representation, plain or compressed, and stick to it.
Under SQLite (5) none of the codec options apply to the store as a whole; only the query column could be dictionary-compressed per row, which is rarely worth the complexity.

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

## Decisions so far

Settled in conversation on 2026-09-08.

1. **Dependencies.**
   C dependencies are not forbidden, but each one has to be built for Windows msvc, macOS, Linux x86-64 and ARM64, and two glibc baselines plus musl, so the bar is "worth the matrix cost", not "pure Rust".
   bestool-psql is nominally standalone but in practice ships inside bestool, which already carries C dependencies including zstd.
   Two SQLite implementations in one process is ruled out: Turso stays, rusqlite does not enter.
2. **Retention is by age, never by size.**
   The audit log does not cull itself to fit a byte budget.
3. **Audit failures never get in the way.**
   The tool is used during incident response, including when the filesystem itself is the incident.
   A failed audit write warns once per session and is otherwise silent; it never refuses or delays the statement.
4. **Tamper evidence is wanted.**
5. **Network and synced filesystems are unsupported.**
   Detect them and warn loudly; refusing to create the store there is acceptable.
6. **History is a startup-time snapshot, deliberately.**
   A user pressing up must see their own last statement, never a concurrent session's.
   An explicit command to refresh history from the store mid-session is a possible later addition, not a requirement.
7. **The export CLI is free to change.**
   What matters is a stable Rust API for reading the log programmatically; the CLI is one consumer of it.
8. **Migration is a one-shot import** of the existing redb main file and any orphan files, streamed so it cannot run out of memory.
9. **Live segments are JSON lines**, one record per line, each record carrying a format version field.
   Greppable on a box mid-incident, and the schema can grow a field without a format bump.
   A versioned binary framing was considered for compactness and rejected: the bytes it would save are the same bytes the codec removes at compaction (repeated keys, decimal timestamps), so it would only shrink the short-lived live tier while costing grep and a hand-maintained format.
   Compactness lives in the compacted tier, not in the live format.
10. **The read API is a module of bestool-psql.**
    The two CLIs and anything in bestool consume it from there.
11. **Live segments are never compressed; compacted files use zstd.**
    A segment is live while the process that created it is alive.
    `ruzstd` is a side quest, not a dependency of this card: benchmark it against `zstd` on this workload, and if it matches, it is a candidate to propose upstream in cargo-binstall, where platform compatibility of the C build has also caused trouble.
12. **Retention is on by default, at 12 months.**
    The organisation's data retention policy sets 12 months for security-sensitive audit logs, which this is.
    Nothing is deleted before it is 12 months old; a configuration option may lengthen it, never shorten it below the default.
13. **Hash chain only, no signing.**
    See the analysis under "Signing" below.
14. **Compaction runs in a throttled, bounded, low-priority background thread at session startup**, and is also exposed as a function on the read API with a CLI subcommand over it.
    Never at session exit.
15. **A closed segment is eligible for compaction once its month has ended.**
    Period files are monthly and each is written exactly once.
    Retention deletes a month file once its month ended more than 12 months ago.
16. **Segments use context records.**
    Session state is written once at segment start and again whenever it changes; query records carry only sequence, timestamp and text.
    Readers carry context forward; export reconstructs flat entries.
17. **Unwritable store directory: warn once, buffer, retry.**
    Records that cannot be written are held in a bounded ring buffer in memory and flushed to a segment if the directory becomes writable during the session.
    The bound keeps memory flat when the directory never becomes writable; the oldest buffered records are dropped first.
    The same path handles a write failure that appears mid-session.
18. **Network or synced filesystem: warn loudly, write anyway.**
    Detection is advisory; the session still records to the configured path.
19. **Startup history is loaded up to a memory budget**, newest first across segments, stopping when the budget is reached.
    The default budget figure is open; a few megabytes of query text is the order of magnitude.
20. **Old redb files are deleted after a successful import.**
    Deletion happens only after the written segment is synced.
    Imported records keep their original timestamps; records carrying an old instance uuid are grouped into a segment per uuid, the rest into a single migration segment.
21. **Both export CLIs stay** as thin wrappers over the read API, and both gain compact and verify subcommands.

## What the decisions prune

- Decision 1 removes rusqlite (5) and, until its cross-process mode stops being experimental, Turso (6).
  LMDB (8) falls to the same matrix-cost test with nothing to show for it.
  With no database left standing, the log-file family is the remaining shelf.
- Decision 2 removes the whole culling apparatus and makes per-period grouping attractive, since age retention becomes "delete files older than N".
- Decision 3 rules out anything a session can block on: no shared write locks in the hot path (12, 10), no daemon (13, 14).
  Per-session segments (2) are the only option where a writer never waits for anyone.
  It also implies a fallback: if the store directory cannot be written at all, the session keeps its history in memory and warns once.
- Decision 4 brings in hash-chained records (4).
- Decision 5 adds a startup check: on Linux read the filesystem type of the store directory, on Windows check the drive type, on macOS read the mount's filesystem name.
- Decision 6 confirms in-memory history loaded from the store tail (19) and rules out live merging.
- Decision 7 means the audit module's public surface is the deliverable, and the two CLIs become thin wrappers over it.
- Decision 8 is straightforward with segments: iterate the redb table with its cursor and write records as they come.

## Narrowed shape

One remaining direction, with its internal choices still open.

**Store.**
A directory of append-only segment files, one per session, each written only by the process that created it.
A segment opens with a context record (users, write mode, supervisor, peers, instance uuid, chain seed) and then carries query records of sequence, timestamp and text, with a context record inserted whenever session state changes (21).
Each record carries the hash of its predecessor (4).
Records are keyed by (instance uuid, sequence) so any set union is a valid merge (15).

**Reads.**
Startup loads the most recent recall-eligible records across segments into rustyline's in-memory history (19), bounded by count.
Export and the Rust API read segments as a time-ordered merge.

**Retention and compaction.**
Closed segments older than a threshold are compacted into per-period files, compressed at that point (24), and periods older than the retention window are deleted.
Compaction is the only step that touches files it did not create, so it runs under a lock and is skipped, not waited for, if the lock is held.

**Open within this shape.**

- Default memory budget for startup history (decision 19).
- Ring buffer capacity for unwritable stores (decision 17).
- Publishing chain heads off-box is a candidate follow-up card, not part of this one.

## Segment lifecycle

Three states, and the transitions are one-way.

- **Live.**
  Owned by a running session, which holds an advisory lock on the file for its whole lifetime.
  The writer never waits on this lock; it only exists so that others can test liveness without guessing from mtime.
  Appended to by exactly one process, never read for compaction.
- **Closed.**
  The owning process has exited.
  A clean exit appends an end record; a crash leaves no end record and a possibly torn last line, which readers skip.
  The liveness test is a try-lock on the file: acquired means closed.
  Closed segments are complete, valid, readable as-is, and may sit uncompacted indefinitely.
- **Compacted.**
  Closed segments past the compaction age are merged into a per-period file, time-ordered and zstd-compressed, and the sources deleted.
  Written to a temporary name, synced, renamed into place, and only then are sources removed, so a crash mid-compaction duplicates rather than loses, and (instance uuid, sequence) keys make duplicates harmless on read.

Startup history only needs the most recent recall-eligible records, so it opens segments newest-first and stops once it has enough.
Uncompacted segment count therefore barely affects startup; compaction is about file count and disk footprint, not read latency.
That makes it safe to run rarely and lazily.

## Compaction placement

| Where | Guarantees it runs | Cost to the user | Notes |
|---|---|---|---|
| Session startup, background thread | On any box that is used at all | Competes with the session for IO at the moment the prompt is wanted; on an incident box this is the worst possible moment | Must be throttled (only when closed-segment count or oldest closed age crosses a threshold), bounded per run, lowest IO priority, try-lock and skip |
| Session exit | Only on clean exits | Delays exit | Abrupt terminal closes and kills never run it; weak guarantee |
| Export tool or read API call | Only when someone audits | None to interactive users | Boxes never audited pile up segments forever; reads must merge them anyway |
| Scheduled job (cron, Task Scheduler, systemd timer) | Reliable cadence | None | Needs deployment per box and per user, since state directories are per user; heavy for a tool that also runs on laptops |

Decision 14 takes the first row as default plus the explicit command, and rules out exit.

## Signing

The question was: what does signing buy over a hash chain when the box is already compromised?

- A hash chain alone detects accidental corruption and careless edits.
  Against an adversary who can write to the directory it proves nothing: they rewrite records and recompute the chain, which is self-verifying.
  It also cannot detect deletion of a whole segment, since segments share no state.
- Signing with a key stored on the same box adds nothing: whoever has the box has the key.
  An HMAC keyed on the machine id is the same thing with a world-readable key.
- What actually resists a compromised box is either forward-secure sealing (journald's model: the sealing key evolves per epoch and old keys are destroyed, so records sealed before the compromise cannot be re-sealed; needs a verification key kept off-box and a clock) or an off-box witness.
  Both are projects in their own right.
- The cheap version of a witness is to publish chain heads: periodically send the latest hash of each segment somewhere the box cannot rewrite.
  A head is a few dozen bytes, and it later proves the log was not rewritten before that point.
  The hash chain is what makes this possible, which is the real reason to build the chain now.

Verdict: hash chain now, no local signing, and publishing chain heads off-box as a candidate follow-up card once there is somewhere to publish them.
