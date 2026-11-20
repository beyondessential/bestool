---
id: CHK-SFH
---

# Hung facility sync

The `sync_facility_hung` healthcheck grades whether a facility server's sync has been running for too long, from the facility server's own record of its sync attempts.
It is one of the doctor's healthchecks; see [CHK](healthchecks.md) for the framework it runs in.

## What it measures

The check reads the facility server's sync run history and takes the most recent attempt by start time, whatever its status.
A deleted run record is not considered.
When that attempt has no completion time, the check measures the minutes since it started.
The attempt's status is not consulted.

> [!NOTE]
> An attempt is given a completion time when it ends, whether it succeeded, failed or was queued.
> An attempt whose server process stopped is never given one.
> Only the most recent attempt is graded so that such a record stops counting once a later attempt has started.

## Reporting

The check reports a single result:

- failed when the most recent attempt has no completion time and started more than sixty minutes ago.
- passed otherwise, including when there is no attempt.

The reason for a failure names whether the attempt is saving pulled changes into the facility database or is still exchanging changes with central.

For an attempt with no completion time, its detail carries the attempt's start time, the minutes since, the session id central issued for it, and when it began and finished saving pulled changes.
The check's detail always carries the fail threshold in minutes.

## Applicability

The check only applies to a facility server.

- It skips when the database cannot be reached, so that a database outage remains something the daemon can alert on rather than something that stops it.
- It skips when the facility server's database has no sync run history, as on a Tamanu version that does not record one.
- It reports broken when its query is faulty, and failed when its query fails for any other reason.
