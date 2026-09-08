---
id: AUD-RET
---

# Compaction and retention

Closed segments accumulate one per session.
Compaction folds them into one file per calendar month so the directory stays small, and retention deletes months older than the retention period.
Neither ever touches a live segment, and neither ever runs where a session could be waiting on it.

## Period files

A period file holds every record from the segments whose month it covers, ordered by timestamp, compressed with zstd.
Records inside a period file are the same records, with the same fields and the same hash chain, as they were in their segments; compaction changes their container, not their content.
Chain verification of a period file verifies each original segment's chain within it.

A closed segment becomes eligible for compaction once the calendar month it was started in has ended.
Records from the current month therefore always remain as plain segments, readable with text tools, and each period file is written exactly once.

## Running compaction

Compaction writes the new period file under a temporary name, synchronises it to disk, renames it into place, and only then deletes the segments it consumed.
An interruption at any point leaves the log with duplicated records rather than missing ones, and readers treat records with the same session identity and sequence number as one record, so a duplicate is harmless.

Compaction takes an advisory lock on the directory for its duration and skips rather than waits when another process holds it.
It only consumes segments whose lock it can acquire, which is what tells it the owning session has exited.

Every session runs compaction in the background at startup, throttled so it does nothing when there is nothing eligible, bounded in how much it processes per run, and at low priority so it does not compete with the prompt.
Compaction is also available on demand through the audit tools ([AUD-API](tools.md)) for operators who prefer to schedule it.

## Retention

Records are retained for at least twelve months by default, the organisation's retention period for security-sensitive audit logs.
The retention period is configurable to longer, never to shorter than the default.
Retention is applied alongside compaction: a period file is deleted once the month it covers ended longer ago than the retention period.
Nothing decides what to keep by how much space it takes.
