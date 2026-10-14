---
id: CHK-SSN
---

# Sync sessions

The `sync_sessions` healthcheck grades whether central is leaving sync sessions open that it should have closed.
It is one of the doctor's healthchecks; see [CHK](healthchecks.md) for the framework it runs in.

> [!NOTE]
> Central closes a session when its device stops connecting, and closes a device's earlier open session when the device starts a new one.
> A device that drops off in the field therefore leaves its session open for a while and then has it closed, with no fault in central.
> The check is about the case where central does not close it.

## What it measures

A session is open while it has no completion time.
The check counts the open sessions, and for each measures how long its device has been silent: the time since the device last connected for that session.

A session's age since it started is not graded.
A session whose device is still connecting is progressing, however long it has run.

Central closes an open session once its device has been silent for central's inactivity window.
The check reads that window from central's configuration, and uses one hour when the configuration does not give one.
A session is left open when its device has been silent for longer than the window, because central should already have closed it.

## Grading

The check's result is:

- passed when no session is left open.
- warning when a session has been left open, and its device has been silent for no more than fifteen minutes beyond the window.
- failed when a session's device has been silent for more than fifteen minutes beyond the window.

A session that has been closed, whether for inactivity or because its device reconnected, is not counted.
A session still within the window is not counted as left open, however many there are.

The check's summary names the number of open sessions and the number left open.
Its detail carries the number of open sessions, the number left open, and the inactivity window in use.

> [!NOTE]
> The fifteen minutes beyond the window is the grace given to central's own sweep, which closes silent sessions every minute.

The check declares a gauge of the number of open sessions, and the phase durations of the most recently completed session ([MET](metrics.md)).
The durations are telemetry and do not affect the result.

## Applicability

The check only applies to a central server.

- It skips when the sync sessions table is not present.
- It reports broken when its query is faulty, and failed when its query fails for any other reason.

Repeated failures of one device are graded by [CHK-STF](sync-tablet-failures.md), not here.
