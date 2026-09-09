# Audit store redesign: test cases

Scenarios that verify the redesigned audit store. Automated cases live in the
`audit` module's unit tests and in `crates/psql/tests/audit_store.rs`; the
manual ones need a real terminal, a real Tailscale, or the passage of a day.

## Record format and framing

- [x] A record serialises with the common fields first and the kind-specific fields flat after them (verifies spec: AUD-STO)
- [x] Every record kind round-trips through serialisation unchanged (verifies spec: AUD-STO)
- [x] A separator never appears inside a record, because JSON escapes control characters (verifies spec: AUD-STO)
- [x] A record's hash covers its JSON text and not its framing bytes (verifies spec: AUD-STO)
- [x] A record whose trailing newline did not survive a crash still parses and hashes as itself (verifies spec: AUD-STO)
- [x] Segment and day file names round-trip through classification, and unrelated names in the directory are ignored (verifies spec: AUD-STO, AUD-RET)

## Writing

- [x] A session writes one segment for the day, opening with a context record and ending with an end record on clean exit (verifies spec: AUD-STO)
- [x] Sequence numbers are contiguous from zero and every record chains onto the one before it (verifies spec: AUD-STO)
- [x] A context change is recorded before the query it applies to, and an unchanged context is not recorded again (verifies spec: AUD-STO)
- [x] A statement's source is recorded as typed, snippet or include (verifies spec: AUD-STO)
- [x] Concurrent sessions write separate segments with no coordination and lose nothing (verifies spec: AUD-STO)
- [x] A live segment is locked and a closed one is not (verifies spec: AUD-STO)
- [x] A session that records nothing leaves no segment behind (verifies spec: AUD-STO)
- [ ] A session live across midnight UTC rolls to a new segment, releases the old one's lock, and carries the chain and numbering across (verifies spec: AUD-STO) — needs a day to pass, or a clock the writer reads through
- [ ] Tailscale peers are sampled once when a segment opens and carried onto that segment's later context records (verifies spec: AUD-STO) — needs a real Tailscale

## Recording never gets in the way

- [x] Statements run whether or not their records could be written, and an unwritable store never panics (verifies spec: AUD)
- [x] A full backlog drops its oldest records and a gap record accounts for them when recording resumes (verifies spec: AUD, AUD-STO)
- [x] The gap takes the first sequence number it covers and names the last (verifies spec: AUD-STO)
- [x] Held records flush in the order they were made, behind the gap (verifies spec: AUD)
- [x] The chain still holds across a gap (verifies spec: AUD-STO)
- [ ] The warning about an unwritable store is printed once and not again for the rest of the session (verifies spec: AUD)
- [ ] A write failure does not delay the prompt in an interactive session (verifies spec: AUD)

## Reading

- [x] Damage in the middle of a file is reported and does not swallow the records after it (verifies spec: AUD-STO)
- [x] Junk before the first separator is reported rather than parsed (verifies spec: AUD-STO)
- [x] A segment reads the same backwards as forwards, including for a record larger than a read chunk (verifies spec: AUD-HIS)
- [x] The merge orders concurrent sessions by time across segments and day files (verifies spec: AUD-API)
- [x] Every entry carries the context of its own session, not of whichever session wrote last (verifies spec: AUD-API)
- [x] A record outside a time range still gets its context from a context record before the range (verifies spec: AUD-API)
- [x] An empty directory reads as an empty log (verifies spec: AUD-API)
- [ ] Reading a year of records with a small limit holds only a bounded window in memory (verifies spec: AUD-API) — needs a large generated store and a memory measurement

## Shell history

- [x] Recall is oldest first across the whole log, including across sessions and day files (verifies spec: AUD-HIS)
- [x] Only statements typed at the prompt are recalled; what a snippet or file expanded to is not (verifies spec: AUD-HIS)
- [x] A statement over the size cutoff is left out of recall but stays in the log (verifies spec: AUD-HIS)
- [x] The memory budget bounds what is recalled, and what survives it is the newest (verifies spec: AUD-HIS)
- [x] A new session recalls what earlier and concurrent sessions recorded (verifies spec: AUD-HIS)
- [x] A session does not see what a concurrent session runs after it started (verifies spec: AUD-HIS)
- [ ] Pressing up and down, and searching backwards, walk the recall set in a real terminal (verifies spec: AUD-HIS)
- [ ] Startup time does not grow noticeably with the number of segments in the directory (verifies spec: AUD-HIS)

## Compaction and retention

- [x] Segments inside the plain-text window are left alone (verifies spec: AUD-RET)
- [x] A day past the window folds into a day file, preserving the records, their bytes and their chain (verifies spec: AUD-RET)
- [x] Several sessions on one day fold into one file in time order (verifies spec: AUD-RET)
- [x] A live segment keeps its whole day out of compaction (verifies spec: AUD-RET)
- [x] Only one day is folded per run, oldest first (verifies spec: AUD-RET)
- [x] An interrupted fold leaves duplicates that read as one record, and a later run tidies up without losing anything (verifies spec: AUD-RET)
- [x] A day file past the retention period is deleted, and one inside it is kept (verifies spec: AUD-RET)
- [x] Compaction skips rather than waits when another process holds the directory (verifies spec: AUD-RET)
- [x] There is nothing worthwhile to do in a fresh directory or one holding only today's segment (verifies spec: AUD-RET)
- [ ] Background compaction at session startup does not compete with the prompt (verifies spec: AUD-RET)

## Tamper evidence

- [x] A clean session verifies, and concurrent sessions each verify (verifies spec: AUD-STO)
- [x] Altering a record in place breaks the chain (verifies spec: AUD-STO)
- [x] Removing a record breaks the chain (verifies spec: AUD-STO)
- [x] Removing a segment from the middle of a session breaks that session's chain (verifies spec: AUD-STO)
- [x] Losing the oldest records reads as retention rather than tampering (verifies spec: AUD-STO)
- [x] A gap is reported without failing the chain: an incomplete log is not an altered one (verifies spec: AUD-STO)
- [x] Unparsable bytes are reported without breaking the records around them (verifies spec: AUD-STO)
- [x] An empty log verifies (verifies spec: AUD-STO)
- [x] Chain heads are exposed for every session (verifies spec: AUD-API)

## Legacy import

- [x] A legacy store is imported into segments and the old files are deleted (verifies spec: AUD-STO)
- [x] Imported records keep their original timestamps and context (verifies spec: AUD-STO)
- [x] The old recall flag becomes a source: typed where it was set, unknown where it was not (verifies spec: AUD-STO)
- [x] Records are grouped into segments by session and day, and imported segments verify (verifies spec: AUD-STO)
- [x] Records with no session identity go to one segment per day under an identity made for the import (verifies spec: AUD-STO)
- [x] Working and orphaned copies are imported alongside the main file (verifies spec: AUD-STO)
- [x] A directory with no legacy store imports nothing (verifies spec: AUD-STO)
- [ ] Importing a store larger than available memory completes without exhausting it (verifies spec: AUD-STO) — needs a large legacy store
- [ ] A tool run on a machine that has not had a session since imports the legacy store and reads it (verifies spec: AUD-STO)

## Tools

- [x] An unfiltered export is the log itself, and reads back and verifies as such (verifies spec: AUD-API)
- [x] A limit takes the newest by default and the oldest on request; zero means everything (verifies spec: AUD-API)
- [x] A narrowed export emits the context record in force before the first query record (verifies spec: AUD-API)
- [x] A time range narrows the export, and dates, datetimes and timestamps all parse (verifies spec: AUD-API)
- [ ] The verify command exits non-zero when a chain does not hold, and zero when every chain holds (verifies spec: AUD-API)
- [ ] The compact command reports what it folded and what it deleted (verifies spec: AUD-API)
- [ ] A closed output pipe ends an export quietly (`bestool-psql-audit export | head -1`) (verifies spec: AUD-API)
- [ ] `bestool audit-psql` and `bestool-psql-audit` behave identically (verifies spec: AUD-API)
- [ ] `jq --seq` reads an export natively, and `grep` still matches record content in a live segment (verifies spec: AUD-STO)

## Whole-store behaviour

- [x] A session records what it ran and a later session reads it back (verifies spec: AUD)
- [x] A crashed session leaves a readable segment with no end record (verifies spec: AUD-STO)
- [x] A directory holding files that are not part of the log is left alone (verifies spec: AUD-STO)
- [ ] A real interactive session against a live database records every statement and meta-command it runs (verifies spec: AUD)
- [ ] Enabling write mode and naming a supervisor appends a context record, and the supervisor is recalled at the next write-mode prompt (verifies spec: AUD-STO)
