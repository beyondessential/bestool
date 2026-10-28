---
id: CHK-STF
---

# Tablet sync failures

The `sync_tablet_failures` healthcheck grades whether any one tablet keeps failing to complete its syncs, from central's point of view.
It is one of the doctor's healthchecks; see [CHK](healthchecks.md) for the framework it runs in and for how a check reports instances.

> [!NOTE]
> A tablet that loses its connection in the field leaves a sync unfinished, and one failure says little about it.
> A tablet that fails again and again points to something an operator can act on, such as its connectivity or the device itself.
> The check therefore reports one instance per tablet, so each is graded and silenced by itself.

## What it measures

A tablet is a mobile device, identified by the device id recorded on its sync sessions.
Facility servers are not measured.
A session that names no device is not measured.

A sync fails when its session:

- was closed by central because its tablet went silent.
- was closed by central because its tablet reconnected and started a new session.
- is open and its tablet has been silent for longer than central's inactivity window, as described in [CHK-SSN](sync-sessions.md).

A sync that completed without being closed by central is not a failure, however long it took.

The check counts each tablet's failed syncs within the last twenty-four hours.
A failure is dated by when central closed the session, or, for a session left open, by when its tablet last connected.

A sync succeeds when its session completed within the window with no errors.
The check notes whether each tablet with a failure also had a successful sync in the window.

## Reporting

The check reports one instance per tablet with at least one failed sync in the window, keyed by the device's id.
A tablet with none has nothing to grade and is not reported.

An instance's result is:

- passed when the tablet has fewer than five failed syncs in the window.
- passed when it has five or more and also a successful sync in the window, since its records are still reaching central.
- failed when it has five or more and no successful sync in the window.

An instance's label is the names of the facilities named by the tablet's most recent failed session, joined with a comma.
Their names are read from central's facility records, and a facility with no record is named by its id.
Its detail carries the number of failed syncs, whether the tablet had a successful sync, and the ids of those facilities.

The check declares a gauge of the number of failed tablets ([MET](metrics.md)).

A deployment with no failed tablet sync in the window reports an empty set of instances.

## Applicability

The check only applies to a central server.

- It skips on a facility server.
- It skips when the database cannot be reached, so that a database outage remains something the daemon can alert on rather than something that stops it.
- It skips when the sync sessions table is not present.
- It reports broken when its query is faulty, and failed when its query fails for any other reason, in either case with no instances.
