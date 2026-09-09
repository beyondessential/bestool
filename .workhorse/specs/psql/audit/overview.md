---
id: AUD
---

# Audit log

bestool-psql records every statement a session runs into a local audit log, together with who ran it and under what conditions.
The log serves two readers with different needs: the session itself, which recalls recent statements as shell history, and an auditor, who later asks what was run on a machine, by whom, and when.
The log is append-only, is written by any number of concurrent sessions without coordinating with each other, and never stands between an operator and their query.

The store format and its integrity guarantees are in [AUD-STO](store.md), shell history in [AUD-HIS](history.md), compaction and retention in [AUD-RET](retention.md), and the programmatic and command-line readers in [AUD-API](tools.md).

## What is recorded

Every statement and meta-command the session executes is recorded, including ones that fail, along with the time it was run to microsecond precision.
Each record is attributed to the operating-system user running the session and the database user the session is connected as.
The session's write mode at the time, and the over-the-shoulder supervisor named when write mode was enabled, are recorded with it.
The active untagged Tailscale peers, each as a device hostname and a login name, are recorded when Tailscale is present, so a session run over a remote shell can be tied to the person at the other end.
Each record is attributable to the session that produced it.
Each statement is recorded with where it came from: typed at the prompt, a named snippet, or an included file given by its absolute path.
Statements that a snippet or an included file ran are recorded like any other, so the log holds what a file actually did rather than only the line that invoked it, and they are kept out of shell history.

## Where the log lives

The log is a directory, given by `--audit-path` or defaulting to the per-user state directory for bestool-psql: `~/.local/state/bestool-psql` on Linux, the local application data directory on macOS and Windows.
The directory belongs to one operating-system user; sessions run by different users write to different directories.
It and everything in it are readable by that user alone, since the log holds the full text of every statement run.

The directory must be on local storage.
When it is found to be on a network or synchronised filesystem the session warns loudly at startup and continues to record there, since a degraded log is better than none.

## Recording never gets in the way

bestool-psql is used during incident response, including incidents where the filesystem itself is failing.
Recording is therefore best effort from the session's point of view: a statement runs whether or not its record could be written, and a write failure never delays the prompt.

When a record cannot be written, the session warns once and thereafter fails silently for the rest of the session.
Records that could not be written are held in memory and written out as soon as a later write succeeds, in their original order.
The in-memory backlog is bounded both by record count and by total size, so a permanently unwritable directory or a pasted megabyte-sized statement cannot grow memory without limit; when either bound is reached the oldest held records are discarded first.
Discarded records are accounted for by a gap record written when recording resumes, so the log states how many records it lost and over what span (see [AUD-STO](store.md)).
A write that fails part-way leaves an incomplete record behind, which readers skip and report; the records on either side of it are unaffected.
