---
id: AUD-STO
---

# Audit store

The audit log is a directory of append-only segment files.
Each segment is written by exactly one session, the one that created it, and by nothing else for as long as that session lives.
Because no two processes ever write the same file, sessions need no locks, no shared state, and no reconciliation: the log as a whole is the union of its segments, and any union of records is a valid log.

## Segments

A session creates its segment when it starts, named after the session's identity, and appends to it for the session's lifetime.
The segment is plain JSON lines: one record per line, so a live segment can be read, tailed and searched with ordinary text tools.
Each record carries a format version so the record shape can grow without readers guessing.

The writing session holds an advisory lock on its segment for the whole session.
The session never waits on this lock; it exists so that another process can tell a live segment from a closed one by attempting the lock, rather than inferring liveness from timestamps.

A segment is closed when its session has exited.
A clean exit appends an end record.
A crash leaves no end record and possibly a torn final line; readers treat a line that does not parse as the end of that segment and skip it.
A closed segment is complete and readable as it stands, and remains so indefinitely until compaction folds it into a period file (see [AUD-RET](retention.md)).

## Records

Every record has the format version, its sequence number within the segment, its timestamp, and the hash of the record before it.
Records are keyed across the whole log by the pair of session identity and sequence number, which cannot collide between sessions and does not depend on clocks agreeing.

Three record kinds exist.

A **context** record carries the session state that applies to all following query records: operating-system user, database user, write mode, over-the-shoulder supervisor, Tailscale peers, and session identity.
The first record of every segment is a context record, and a new context record is appended whenever any of that state changes, such as write mode being enabled or a supervisor being named.

A **query** record carries the statement text and whether the statement is eligible for shell recall.
Everything else about a query record is found by carrying forward the most recent context record before it.

An **end** record marks a clean session exit.

```jsonl
{"v":1,"seq":0,"ts":"2026-09-08T03:14:15.926535Z","prev":"","kind":"context","sys_user":"felix","db_user":"tamanu","writemode":false,"ots":null,"tailscale":[{"device":"laptop","user":"felix@example.com"}],"instance":"7d2c…"}
{"v":1,"seq":1,"ts":"2026-09-08T03:14:22.000481Z","prev":"9f86d0…","kind":"query","query":"select count(*) from patients;","recall":true}
{"v":1,"seq":2,"ts":"2026-09-08T03:15:01.114202Z","prev":"e3b0c4…","kind":"end"}
```

## Tamper evidence

Each record's `prev` field is the SHA-256 hash of the previous record's line exactly as written to disk, so verification is a byte-level read and needs no canonical re-encoding.
The first record of a segment has an empty `prev`.
A segment verifies when every record's `prev` matches the hash of the line before it and the sequence numbers are contiguous from zero.
Verification reports the first record at which the chain breaks.

The chain shows that a segment has not been edited or reordered since it was written.
It is the input to an off-box witness that would show a segment has not been rewritten wholesale; that witness is outside this spec.

## Legacy stores

A directory containing a store in the earlier single-file database format, including any working-copy or orphaned files that format left behind, is imported into segments the first time a session opens it.
Import streams records from the old files rather than loading them whole, so it completes within flat memory regardless of their size.
Imported records keep their original timestamps.
Records that carry a session identity are grouped into a segment per session; the rest go into a single import segment.
The old files are deleted only after the new segments have been written and synchronised to disk.
