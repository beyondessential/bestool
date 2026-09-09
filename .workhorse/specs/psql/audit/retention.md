---
id: AUD-RET
---

# Compaction and retention

Closed segments accumulate, one per session per day that session was active.
Compaction folds each day's segments into a single day file so the directory stays small, and retention deletes days older than the retention period.
Neither ever touches a live segment, and neither ever runs where a session could be waiting on it.

## Day files

A day file holds every record from the segments covering that UTC day, ordered by timestamp, compressed with zstd.
Records inside a day file are the same records, with the same fields, framing and hash chain as they had in their segments; compaction changes their container, not their content, and decompressing a day file gives back the bytes it folded.
Verifying a session's chain follows its records through the day files that hold them, in the same way as through its segments (see [AUD-STO](store.md)).

A day's segments become eligible for compaction once that day ended longer ago than the plain-text window.
The window is a span of days rather than a calendar boundary, so the same recent stretch of the log is readable with ordinary text tools whatever the date, and each day file is written exactly once.

## Running compaction

Compaction writes the new day file under a temporary name, synchronises it to disk, renames it into place, and only then deletes the segments it consumed.
An interruption at any point leaves the log with duplicated records rather than missing ones, and readers treat records with the same session identity and sequence number as one record, so a duplicate is harmless.

Compaction takes an advisory lock on the directory for its duration and skips rather than waits when another process holds it.
It only consumes segments whose own lock it can acquire, which is what tells it nothing is writing them: a session that has run for a week keeps only its current day open, so its earlier days compact while it is still live.

Every session runs compaction in the background at startup, throttled so it does nothing when there is nothing eligible, bounded in how many days it processes per run, and at low priority so it does not compete with the prompt.
Compaction is also available on demand through the audit tools ([AUD-API](tools.md)) for operators who prefer to schedule it.

## Retention

Records are retained for twelve months, the organisation's retention period for security-sensitive audit logs.
Retention is applied alongside compaction: a day file is deleted once its day ended longer ago than the retention period.
Nothing decides what to keep by how much space it takes.
