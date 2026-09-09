---
id: AUD-API
---

# Audit tools

The audit log is read through a Rust API in bestool-psql, and that API is what the command-line tools, and anything else in bestool, consume.
The API is the stable surface; the command-line tools are thin wrappers over it.

## Read API

The API opens an audit directory and reads it as one time-ordered stream of records across all segments and day files, with each query record's context carried forward so the caller sees flat entries.
Reading is streaming: a caller iterating a year of records never holds more than a bounded window in memory.
The stream can be restricted to a time range and limited to the newest or oldest N entries.

The API verifies hash chains, reporting per session whether the chain holds and where it first breaks (see [AUD-STO](store.md)).
It exposes the current chain head of every session, which is what an off-box witness would publish.
It runs compaction and retention on demand (see [AUD-RET](retention.md)).

## Command-line tools

`bestool-psql-audit` and `bestool audit-psql` are the same tool reached two ways.
Both take an audit directory, defaulting to the same location the session uses.

The export command writes entries as JSON lines to standard output, one flat entry per line with its timestamp in RFC 3339 form, and accepts a time range, a limit, and a choice of newest or oldest first.
A closed output pipe ends the export quietly.

The verify command checks every session's chain across the segments and day files that hold it, reports any chain that does not hold, and exits non-zero if any fails.

The compact command runs compaction and retention once and reports what it folded and what it deleted.
