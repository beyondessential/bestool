---
id: AUD-API
---

# Audit tools

The audit log is read through a Rust API in bestool-psql, and that API is what the command-line tools, and anything else in bestool, consume.
The API is the stable surface; the command-line tools are thin wrappers over it.

## Read API

The API opens an audit directory and reads it as one time-ordered stream of records across all segments and day files.
The stream is offered two ways: the records as they are stored, which is what the export command writes out, and flat entries, where each query record carries the context in force at it so a caller needs no state of its own.
Reading is streaming: a caller iterating a year of records never holds more than a bounded window in memory.
The stream can be restricted to a time range and limited to the newest or oldest N entries.

The API verifies hash chains, reporting per session whether the chain holds, where it first breaks, and the gap records and unparsable bytes it passed (see [AUD-STO](store.md)).
It exposes the current chain head of every session, which is what an off-box witness would publish.
It runs compaction and retention on demand (see [AUD-RET](retention.md)).

## Command-line tools

`bestool-psql-audit` and `bestool audit-psql` are the same tool reached two ways.
Both take an audit directory, defaulting to the same location the session uses.

The export command writes records to standard output in the shape and framing they have in the store: like compaction, export changes their container and not their content, so an unfiltered export is the log itself, merged into time order and decompressed, and verifies as such.
It accepts a time range, a limit, and a choice of newest or oldest first.
When a filter narrows the output, the context record in force at the start of it is emitted first, so every query record in the output can still be attributed.
A closed output pipe ends the export quietly.

The verify command checks every session's chain across the segments and day files that hold it, and exits non-zero if any chain does not hold.
It reports gap records and unparsable bytes wherever it meets them, so an incomplete log is told apart from an altered one.

The compact command runs compaction and retention once and reports what it folded and what it deleted.
