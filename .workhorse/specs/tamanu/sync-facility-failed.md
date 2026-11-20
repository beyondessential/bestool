---
id: CHK-SFF
---

# Failed facility sync

The `sync_facility_failed` healthcheck grades whether a facility server's most recent sync failed, from the facility server's own record of its sync attempts.
It is one of the doctor's healthchecks; see [CHK](healthchecks.md) for the framework it runs in.

## What it measures

The check reads the facility server's sync run history and takes the most recent attempt, by start time, that ended as succeeded or failed.
An attempt that central queued for a later turn is not considered, and neither is one that is still running.
A deleted run record is not considered.

> [!NOTE]
> A queued attempt says nothing about whether sync works, and one still running has no outcome yet.
> An attempt that never ends is graded by [CHK-SFH](sync-facility-hung.md).

## Reporting

The check reports a single result:

- failed when the most recent attempt it considers failed, with that attempt's error message in its reason.
- passed when that attempt succeeded, or when there is no attempt to consider.

Its detail carries that attempt's start time, completion time and error message.

## Applicability

The check only applies to a facility server.

- It skips when the database cannot be reached, so that a database outage remains something the daemon can alert on rather than something that stops it.
- It skips when the facility server's database has no sync run history, as on a Tamanu version that does not record one.
- It reports broken when its query is faulty, and failed when its query fails for any other reason.
