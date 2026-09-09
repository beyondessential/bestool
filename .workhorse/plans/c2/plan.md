# Audit store redesign: implementation plan

The design is specified in `.workhorse/specs/psql/audit/`: [AUD](../../specs/psql/audit/overview.md), [AUD-STO](../../specs/psql/audit/store.md), [AUD-HIS](../../specs/psql/audit/history.md), [AUD-RET](../../specs/psql/audit/retention.md), [AUD-API](../../specs/psql/audit/tools.md).
The problem exploration, technique sweep and the reasoning behind each decision are in this file's git history up to commit `adeed998`.

## Implementation notes

Things the specs deliberately leave to the implementation, recorded here so they are chosen once.

- **Defaults.** Startup recall budget 4 MiB of query text; per-record recall cutoff 10 KiB; unwritable-store backlog 1000 records and 16 MiB; retention 12 months; plain-text window 30 days.
- **Hash.** SHA-256 over the previous line's raw bytes including nothing after the newline. Pending confirmation, since the plan only said "hash" and the spec had to name one.
- **File naming.** Segments `audit-<YYYY-MM-DD>-<instance-uuid>.jsonl`; day files `audit-<YYYY-MM-DD>.jsonl.zst`. Legacy files are `audit-main.redb`, `audit-working-*.redb`, `audit-orphaned-*.redb`.
- **Day boundary.** UTC, matching the record timestamps. A segment rolls at midnight UTC and the writer drops the previous segment's lock as it does.
- **Codec.** The `zstd` crate, already in bestool's tree via the self-update downloader. `ruzstd` is a side quest: benchmark against `zstd` on compacted audit data and, if it holds up, propose it upstream in cargo-binstall. Not part of this card.
- **Locks.** Advisory lock on the segment file held by its writer until it rolls to the next day or exits; advisory lock on a directory-level lock file held by compaction. Writers never take the directory lock. Use a small cross-platform crate rather than raw fcntl/LockFileEx.
- **Network filesystem detection.** Linux: filesystem type of the store directory via statfs (nfs, cifs, smb, fuse variants, 9p). Windows: drive type of the path is remote. macOS: mount filesystem name via statfs. Advisory only; the session warns and continues.
- **Startup history read order.** Open segments and day files newest-first by file, which the date in every filename gives directly, and within a file read from the end where the format allows, stopping when the budget is met. Segment count therefore barely affects startup.
- **Compaction throttle.** Run only when at least one closed segment sits outside the plain-text window. Process at most one day per session start. Lowest IO and thread priority the platform offers.
- **Legacy import** needs redb to read the old files, so the dependency stays for the reader only. Group by the old `instance_id` where present and by day within that; otherwise one import segment per day.
- **Turso** stays out until its cross-process mode drops the experimental label; re-evaluate then, not before.
- **README.** The `--audit-path` row still describes the old single-file default and needs its text corrected as part of this work.

## Checklist

### Store

- [ ] Segment writer: create the day's segment on first record, take its advisory lock, write the opening context record, append query records, append the end record on clean exit.
- [ ] Day rollover: at the first record past midnight UTC, close and unlock the current segment and open the next, carrying the chain and sequence numbers across.
- [ ] Hash chain: compute `prev` from the previous line's raw bytes; empty only for the session's very first record.
- [ ] Context records on state change: hook write-mode toggles, supervisor changes and any other context field so a new context record is appended before the next query record.
- [ ] Write-failure path: warn once, bounded backlog by count and bytes, oldest dropped first, flush in order on the next successful write.
- [ ] Network filesystem detection at open with a loud warning.
- [ ] Legacy import: stream the redb tables into segments grouped by old instance id, sync, then delete the old files.

### Reader

- [ ] Segment parser: JSON lines with format version, torn or unparsable final line ends the segment.
- [ ] Day file parser: zstd-compressed JSON lines.
- [ ] Merged reader: time-ordered k-way merge across segments and day files, context carried forward, dedup by (instance, seq), time-range filter, newest/oldest limit, streaming with a bounded window.
- [ ] Chain verification per session, following its records across segments and day files in date order, reporting the first break and treating the oldest kept record's `prev` as unverifiable; chain heads per session.

### Shell history

- [ ] Recall set builder: newest-first across files to the memory budget, skipping records above the size cutoff and records marked not for recall.
- [ ] Replace the rustyline history implementation with an in-memory history seeded from the recall set and appended to as the session runs.

### Compaction and retention

- [ ] Eligibility: closed segments (lock acquirable) whose day ended longer ago than the plain-text window.
- [ ] Compaction: write the day file to a temporary name, sync, rename, then delete consumed segments; directory lock held throughout, skip if held.
- [ ] Retention: delete day files whose day ended longer ago than the retention period; period configurable to longer only.
- [ ] Background compaction at session startup with the throttle and bound above.

### Tools

- [ ] Read API surface in the audit module: open, stream entries with filters, verify, chain heads, compact.
- [ ] Export, verify and compact commands in both `bestool-psql-audit` and `bestool audit-psql`; drop the orphan flag.
- [ ] Retention period configuration option.

### Removal

- [ ] Delete the working-copy, sync, orphan-recovery, index-table and culling code and the redb-backed history implementation.
- [ ] Correct the README row for `--audit-path`.
