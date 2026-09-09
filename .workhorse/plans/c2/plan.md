# Audit store redesign: implementation plan

The design is specified in `.workhorse/specs/psql/audit/`: [AUD](../../specs/psql/audit/overview.md), [AUD-STO](../../specs/psql/audit/store.md), [AUD-HIS](../../specs/psql/audit/history.md), [AUD-RET](../../specs/psql/audit/retention.md), [AUD-API](../../specs/psql/audit/tools.md).
The problem exploration, technique sweep and the reasoning behind each decision are in this file's git history up to commit `adeed998`.

## Implementation notes

Things the specs deliberately leave to the implementation, recorded here so they are chosen once.

- **Defaults.** Startup recall budget 4 MiB of query text; per-record recall cutoff 10 KiB; unwritable-store backlog 1000 records and 16 MiB; plain-text window 30 days. The twelve-month retention period is fixed by the spec, not a default.
- **Hash.** SHA-256 over the previous record's JSON text: the bytes between its `0x1E` separator and its newline, neither of which is hashed. Framing is transport, content is hashed, which is what lets a record keep its hash through compaction and export.
- **Framing.** RFC 7464 JSON text sequences, `0x1E` before each record and `0x0A` after. `jq --seq` reads it natively; plain `grep` still matches record content, but `^`-anchored patterns have to allow for the leading separator.
- **File naming.** The live and compacted names are specified in [AUD-STO](../../specs/psql/audit/store.md) and [AUD-RET](../../specs/psql/audit/retention.md); dates are `YYYY-MM-DD` and the session identity is the instance UUID. Legacy files to recognise on import are `audit-main.redb`, `audit-working-*.redb`, `audit-orphaned-*.redb`.
- **Day boundary.** UTC, matching the record timestamps. A segment rolls at midnight UTC and the writer drops the previous segment's lock as it does.
- **Codec.** The `zstd` crate, already in bestool's tree via the self-update downloader. `ruzstd` is a side quest: benchmark against `zstd` on compacted audit data and, if it holds up, propose it upstream in cargo-binstall. Not part of this card.
- **Locks.** Advisory lock on the segment file held by its writer until it rolls to the next day or exits; advisory lock on a directory-level lock file held by compaction. Writers never take the directory lock. Use a small cross-platform crate rather than raw fcntl/LockFileEx.
- **Tailscale sampling.** `tailscale status` is a subprocess, and the current implementation spawns it on every entry. Sample once per segment instead, at session start and each rollover, and reuse that set for the segment's later context records. The recorded set is then who was reachable when the segment opened; a session handed to someone else inside a shared tmux keeps the peers from the open until the next rollover, which is accepted.
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
- [ ] Hash chain: compute `prev` at write time from the previous whole record's JSON text; empty only for the session's very first record.
- [ ] Gap record: on resuming after discards, emit a gap taking the first discarded sequence number and carrying the count, last number and time span; then flush the surviving backlog behind it.
- [ ] Context records on state change: hook write-mode toggles, supervisor changes and any other context field so a new context record is appended before the next query record.
- [ ] Sample Tailscale peers at segment open only, and carry that set onto the segment's later context records; drop the per-entry `get_active_peers` call.
- [ ] Write-failure path: warn once, bounded backlog by count and bytes, oldest dropped first, flush in order on the next successful write.
- [ ] Network filesystem detection at open with a loud warning.
- [ ] Legacy import: stream the redb tables into segments grouped by old instance id, sync, then delete the old files. Runs under the directory lock, from tools as well as sessions, and is skipped when the lock is held.

### Reader

- [ ] Segment parser: split on `0x1E`, parse each record, and on failure report the skipped bytes and resume at the next separator rather than ending the file.
- [ ] Day file parser: zstd-compressed JSON lines.
- [ ] Merged reader: time-ordered k-way merge across segments and day files, dedup by (instance, seq), time-range filter, newest/oldest limit, streaming with a bounded window. Two surfaces over the one merge: stored records for export, and context-carried-forward flat entries for the recall set and other callers.
- [ ] Filtered export emits the context record in force at the start of the output before the first query record.
- [ ] Chain verification per session, following its records across segments and day files in date order, reporting the first break and treating the oldest kept record's `prev` as unverifiable; chain heads per session.

### Shell history

- [ ] Recall set builder: newest-first across files to the memory budget, skipping records above the size cutoff and records marked not for recall.
- [ ] Replace the rustyline history implementation with an in-memory history seeded from the recall set and appended to as the session runs.

### Compaction and retention

- [ ] Eligibility: closed segments (lock acquirable) whose day ended longer ago than the plain-text window.
- [ ] Compaction: write the day file to a temporary name, sync, rename, then delete consumed segments; directory lock held throughout, skip if held.
- [ ] Retention: delete day files whose day ended longer ago than the fixed twelve-month period.
- [ ] Background compaction at session startup with the throttle and bound above.

### Tools

- [ ] Read API surface in the audit module: open, stream entries with filters, verify, chain heads, compact.
- [ ] Export, verify and compact commands in both `bestool-psql-audit` and `bestool audit-psql`; drop the orphan flag.

### Removal

- [ ] Delete the working-copy, sync, orphan-recovery, index-table and culling code and the redb-backed history implementation.
- [ ] Delete the plain `~/.psql_history` import (`import_psql_history`): transitional, long since served.
- [ ] Correct the README row for `--audit-path`.
