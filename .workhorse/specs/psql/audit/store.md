---
id: AUD-STO
---

# Audit store

The audit log is a directory of append-only segment files.
Each segment is written by exactly one session, the one that created it, and by nothing else for as long as that session holds it open.
Because no two processes ever write the same file, sessions need no locks, no shared state, and no reconciliation: the log as a whole is the union of its segments, and any union of records is a valid log.

## Segments

A session writes its records into one segment per UTC day, named after the session's identity and the date it covers.
A segment is created when the session first records something on that day, so a session left open over a quiet night leaves no file for it.
Most sessions are shorter than a day and so have exactly one segment.
Because a segment never spans a date boundary, every closed segment falls entirely within one day.

The segment is a JSON text sequence: each record is one line of JSON, preceded by an ASCII record separator (`0x1E`) and followed by a newline, the framing of RFC 7464.
A live segment can be read, tailed and searched with ordinary text tools, and parsed by anything that understands JSON lines or JSON sequences.
Each record carries a format version so the record shape can grow without readers guessing.

The separator never occurs inside a record, because JSON escapes control characters, so it marks the start of a record unambiguously.
A reader that meets bytes it cannot parse as a record skips to the next separator, reports what it skipped, and carries on with the records after it.
Damage is therefore confined to the record it lands in: a half-written record cannot swallow the records that follow it, and cannot be mistaken for the end of the file.

The writing session holds an advisory lock on its current segment, and releases it when it rolls over to the next day, so a lock that can be acquired always means nothing is writing that file.
The session never waits on this lock; it exists so that another process can tell a live segment from a closed one by attempting the lock, rather than inferring liveness from timestamps.

A segment is closed once its session has rolled past it or exited.
A clean exit appends an end record to the session's last segment.
Rolling over to a new day does not, because the next segment's first record carries the hash of this one's last line and so shows the session continued.
A crash leaves no end record and possibly a torn final record, which readers discard.
A closed segment is complete and readable as it stands, and remains so until compaction folds it into a day file (see [AUD-RET](retention.md)).

## Records

Every record has the format version, its sequence number within the session, its timestamp, and the hash of the record before it.
Records are keyed across the whole log by the pair of session identity and sequence number, which cannot collide between sessions and does not depend on clocks agreeing.
A sequence number is assigned when a record is made rather than when it reaches disk, so a record that is never written leaves a hole in the numbering.

Four record kinds exist.

A **context** record carries the session state that applies to all following query records: operating-system user, database user, write mode, over-the-shoulder supervisor, Tailscale peers, and session identity.
The first record of every segment is a context record, and a new context record is appended whenever any of that state changes, such as write mode being enabled or a supervisor being named.

The Tailscale peers are the exception: they are sampled once when a segment opens, at session start and again at each rollover, and every context record in that segment carries the set sampled then.
The whole set of active peers is recorded because which one of them owns the session cannot be determined, and it stands as who was reachable when the segment opened.

A **query** record carries the statement text and whether the statement is eligible for shell recall.
Everything else about a query record is found by carrying forward the most recent context record before it.

An **end** record marks a clean session exit.

A **gap** record stands in for records that were made but never written, because the in-memory backlog had to discard them (see [AUD](overview.md)).
It carries how many records were lost, the last sequence number they held, and the span of time they covered, and it takes the first of the sequence numbers it covers so the numbering in a segment stays in order.
The log therefore says where it is incomplete, rather than leaving a silent hole that reads as tampering.

A segment, with its framing bytes left out for legibility:

```jsonl
{"v":1,"seq":0,"ts":"2026-09-08T03:14:15.926535Z","prev":"","kind":"context","sys_user":"felix","db_user":"tamanu","writemode":false,"ots":null,"tailscale":[{"device":"laptop","user":"felix@example.com"}],"instance":"7d2c…"}
{"v":1,"seq":1,"ts":"2026-09-08T03:14:22.000481Z","prev":"9f86d0…","kind":"query","query":"select count(*) from patients;","recall":true}
{"v":1,"seq":2,"ts":"2026-09-08T03:44:09.550118Z","prev":"e3b0c4…","kind":"gap","lost":87,"through":88,"from":"2026-09-08T03:14:30.104881Z","to":"2026-09-08T03:44:02.771290Z"}
{"v":1,"seq":89,"ts":"2026-09-08T03:44:09.551002Z","prev":"5f2b81…","kind":"query","query":"select now();","recall":true}
{"v":1,"seq":90,"ts":"2026-09-08T03:45:01.114202Z","prev":"a1d4f0…","kind":"end"}
```

## Tamper evidence

Each record's `prev` field is the SHA-256 hash of the previous record's JSON text: the bytes between its separator and its newline, exactly as written, so verification is a byte-level read that needs no canonical re-encoding.
The framing bytes are outside the hash, so a record hashes identically wherever it is held, and a record whose trailing newline did not survive a crash still hashes as itself.
`prev` names the previous record rather than the previous bytes on disk: it is fixed when the record is written, and bytes that no reader can parse as a record take no part in the chain.

The chain runs for the life of the session rather than the life of a file: only the first record a session ever writes has an empty `prev`, and the first record of each later segment carries the hash of the last record of the segment before it.
A session verifies when, reading its segments in date order, every record's `prev` matches the hash of the record before it, and the sequence numbers are contiguous from zero with gap records accounting for any numbers missing.
Verification reports the first record at which the chain breaks, along with every gap record and every stretch of unparsable bytes it passed.
Removing a whole segment therefore breaks the chain of the session it belonged to, rather than passing unnoticed, and so does altering or truncating a record in place.
A half-written record left behind by a failed write does not, because the record that follows it chains onto the last record that was written whole.

Where retention has already deleted a session's earlier records, verification starts from the oldest records kept and reports their `prev` as unverifiable rather than broken.

The chain shows that a session's records have not been edited, reordered or removed since they were written.
It is the input to an off-box witness that would show they have not been rewritten wholesale; that witness is outside this spec.

## Legacy stores

A directory containing a store in the earlier single-file database format, including any working-copy or orphaned files that format left behind, is imported into segments the first time any process opens it, session or tool alike, so an auditor reading a machine that has not run a session since sees what a session would.
Import runs under the same directory lock as compaction ([AUD-RET](retention.md)); a process that cannot take the lock reads what is already there and leaves the import to whoever holds it.
Import streams records from the old files rather than loading them whole, so it completes within flat memory regardless of their size.
Imported records keep their original timestamps.
Records that carry a session identity are grouped into segments by session and day; the rest go into an import segment per day.
The old files are deleted only after the new segments have been written and synchronised to disk.
