---
id: AUD-HIS
---

# Shell history

Shell history in bestool-psql is a view over the audit log, not a separate store.
Pressing up and down, and searching backwards, walk a recall set held in memory for the session.

## The recall set

At startup the session builds its recall set from the audit log: recall-eligible query records, newest first, across all segments and period files, stopping once a memory budget is reached.
Individual records above a size cutoff are left out of the recall set entirely; they stay in the audit log but a single very large pasted statement never consumes the budget on its own.
The budget and cutoff keep startup cost and memory flat no matter how busy the user has been.

Statements the session runs are appended to its recall set as it goes, in the order they were run.
The recall set is otherwise fixed for the life of the session: an operator who presses up sees the statement they just ran, never one that a concurrent session happened to run in the meantime.
A new session sees everything recorded before it started, including by sessions that were live at the time.

## Recall eligibility

A query record is recall-eligible unless it was marked otherwise when written.
Statements that came from a snippet or an included file are marked not for recall, as described in [AUD](overview.md).
