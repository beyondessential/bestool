# Audit store redesign: implementation plan

The design is specified in `.workhorse/specs/psql/audit/`: [AUD](../../specs/psql/audit/overview.md), [AUD-STO](../../specs/psql/audit/store.md), [AUD-HIS](../../specs/psql/audit/history.md), [AUD-RET](../../specs/psql/audit/retention.md), [AUD-API](../../specs/psql/audit/tools.md).
The problem exploration and technique sweep are in this file's git history up to commit `adeed998`.
The design settled after that: per-session-day segments, framing, gap records and query sources were worked out on the card and their reasoning is carried in the specs themselves.

## Implementation notes

Things the specs deliberately leave to the implementation, recorded here so they are chosen once.

- **Defaults.** Startup recall budget 4 MiB of query text; per-record recall cutoff 10 KiB; unwritable-store backlog 1000 records and 16 MiB. The twelve-month retention period and the fifteen-day plain-text window are both fixed by the spec, not defaults.
- **Hash.** SHA-256 over the previous record's JSON text: the bytes between its `0x1E` separator and its newline, neither of which is hashed. Framing is transport, content is hashed, which is what lets a record keep its hash through compaction and export.
- **Framing.** RFC 7464 JSON text sequences, `0x1E` before each record and `0x0A` after. `jq --seq` reads it natively; plain `grep` still matches record content, but `^`-anchored patterns have to allow for the leading separator.
- **File naming.** The live and compacted names are specified in [AUD-STO](../../specs/psql/audit/store.md) and [AUD-RET](../../specs/psql/audit/retention.md); dates are `YYYY-MM-DD` and the session identity is the instance UUID. Legacy files to recognise on import are `audit-main.redb`, `audit-working-*.redb`, `audit-orphaned-*.redb`.
- **Day boundary.** UTC, matching the record timestamps. A segment rolls at midnight UTC and the writer drops the previous segment's lock as it does.
- **Codec.** The `zstd` crate, already in bestool's tree via the self-update downloader. `ruzstd` is a side quest: benchmark against `zstd` on compacted audit data and, if it holds up, propose it upstream in cargo-binstall. Not part of this card.
- **Gap record timestamp.** A gap takes the time of the last record it covers, not the time recording resumed. The held records that survived the discards were made before it and are written behind it, so this is what keeps write order, sequence order and time order in agreement, which is in turn what lets a time-ordered export verify.
- **Attribution.** A segment's name gives the session that wrote it, so records read from a segment are attributed outright. A day file interleaves sessions and names none, so its records are attributed by following `prev` back to a chain head, bootstrapped from the `instance` on each context record. Attribution therefore survives a record being altered or removed: the records after it are still known to belong to the session whose chain they broke.
- **Recall ordering.** Files are read newest first for the budget, then what was collected is sorted by timestamp, so concurrent sessions on one day recall in the order they ran rather than in filename order. The budget bounds how much there is to sort.
- **Compaction priority.** `thread-priority` sets the compaction thread to the platform minimum. IO priority is left alone: there is no obvious cross-platform crate for it, and the machines this runs on do not generally have a prioritisable IO scheduler enabled.
- **Locks.** Lock on the segment file held by its writer until it rolls to the next day or exits; lock on a directory-level lock file held by compaction. Writers never take the directory lock. `fs4` rather than raw fcntl/LockFileEx.
- **Shared versus exclusive.** A writer takes a *shared* lock on its segment and everything else tries an *exclusive* one. The two are not interchangeable: Windows byte-range locks are enforced rather than advisory, so a writer holding exclusively would stop every reader of a live segment, which the format exists to allow. An exclusive attempt still fails while the share is held, so liveness is reported the same on both platforms. Compaction takes its segments exclusively and reads them back through the very handle it locked, because opening a second one would be refused on Windows; the cost is that on Windows a concurrent read of the specific segments being folded can fail for the length of the fold.
- **Timestamps are not unique.** macOS hands out a coarse enough clock that a context record and the query after it can share one, so anything using a record's position in the log as a boundary orders by sequence number as well as timestamp.
- **Tailscale sampling.** `tailscale status` is a subprocess, and the current implementation spawns it on every entry. Sample once per segment instead, at session start and each rollover, and reuse that set for the segment's later context records. The recorded set is then who was reachable when the segment opened; a session handed to someone else inside a shared tmux keeps the peers from the open until the next rollover, which is accepted.
- **Startup history read order.** Open segments and day files newest-first by file, which the date in every filename gives directly, and within a file read from the end where the format allows, stopping when the budget is met. Segment count therefore barely affects startup.
- **Compaction throttle.** Run only when at least one closed segment sits outside the plain-text window. Process at most one day per session start. Lowest IO and thread priority the platform offers.
- **Legacy import** needs redb to read the old files, so the dependency stays for the reader only. Group by the old `instance_id` where present and by day within that; otherwise one import segment per day.
- **Turso** stays out until its cross-process mode drops the experimental label; re-evaluate then, not before.
- **README.** The `--audit-path` row still describes the old single-file default and needs its text corrected as part of this work.
- **Deferred.** The network-filesystem warning is not part of this card; it is a follow-up in [the breakdown](../../breakdowns/c2/breakdown.md). The store works on a network filesystem, just without the warning.

## Checklist

### Store

- [x] Segment writer: create the day's segment on first record, take its advisory lock, write the opening context record, append query records, append the end record on clean exit.
- [x] Day rollover: at the first record past midnight UTC, close and unlock the current segment and open the next, carrying the chain and sequence numbers across.
- [x] Hash chain: compute `prev` at write time from the previous whole record's JSON text; empty only for the session's very first record.
- [x] Sequence numbers assigned when a record is made, not when it lands, so a discarded record leaves a hole.
- [x] Gap record: on resuming after discards, emit a gap taking the first discarded sequence number and carrying the count, last number and time span; then flush the surviving backlog behind it.
- [x] Query record source: `"source":"typed"`, or `"source":"snippet"` with the snippet name, or `"source":"include"` with the absolute path as resolved for opening. Flat fields rather than a nested object, so `source` is always a string and stays greppable. Replaces the `recall` boolean, which readers now derive.
- [x] Record statements run from snippets and included files at all. Expansion in `repl/snippets.rs` and `repl/include.rs` loops over `action.dispatch(...)`, which bypasses the recording in `ReplAction::handle`, so today only the invocation line is logged and the statements it runs are not. `from_snippet_or_include` is set around that loop but nothing on the path calls `add_entry`, so the not-for-recall marking has never applied to anything.
- [x] Context records on state change: hook write-mode toggles, supervisor changes and any other context field so a new context record is appended before the next query record.
- [x] Sample Tailscale peers at segment open only, and carry that set onto the segment's later context records; drop the per-entry `get_active_peers` call.
- [x] Write-failure path: warn once, bounded backlog by count and bytes, oldest dropped first, flush in order on the next successful write.
- [x] Legacy import: stream the redb tables into segments grouped by old instance id, sync, then delete the old files. Runs under the directory lock, from tools as well as sessions, and is skipped when the lock is held. Map the old `recall` boolean to a source: true to `typed`, false to `unknown`. Records with no old instance id go to per-day import segments under an identity generated for the import, so the segment naming rule holds for them too.

### Reader

- [x] Segment parser: split on `0x1E`, parse each record, and on failure report the skipped bytes and resume at the next separator rather than ending the file.
- [x] Day file parser: zstd wrapping the same framed records as a segment, so the same record reader runs over both.
- [x] Merged reader: time-ordered k-way merge across segments and day files, dedup by (instance, seq), time-range filter, newest/oldest limit, streaming with a bounded window. Two surfaces over the one merge: stored records for export, and context-carried-forward flat entries for the recall set and other callers.
- [x] Filtered export emits the context record in force at the start of the output before the first query record.
- [x] Chain verification per session, following its records across segments and day files in date order, reporting the first break, the gap records and unparsable bytes passed, and treating the oldest kept record's `prev` as unverifiable; chain heads per session.

### Shell history

- [x] Recall set builder: newest-first across files to the memory budget, skipping records above the size cutoff and any whose source is not the prompt.
- [x] Replace the rustyline history implementation with an in-memory history seeded from the recall set and appended to as the session runs.

### Compaction and retention

- [x] Eligibility: closed segments (lock acquirable) whose day ended longer ago than the plain-text window.
- [x] Compaction: write the day file to a temporary name, sync, rename, then delete consumed segments; directory lock held throughout, skip if held.
- [x] Retention: delete day files whose day ended longer ago than the fixed twelve-month period.
- [x] Background compaction at session startup with the throttle and bound above.

### Tools

- [x] Read API surface in the audit module: open, stream entries with filters, verify, chain heads, compact.
- [x] Export, verify and compact commands in both `bestool-psql-audit` and `bestool audit-psql`; drop the orphan flag.

### Removal

- [x] Delete the working-copy, sync, orphan-recovery, index-table and culling code and the redb-backed history implementation.
- [x] Delete the plain `~/.psql_history` import (`import_psql_history`): transitional, long since served.
- [x] Correct the README row for `--audit-path`.
