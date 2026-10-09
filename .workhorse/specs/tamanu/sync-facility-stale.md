---
id: CHK-SFS
---

# Stale facility sync

The `sync_facility_stale` healthcheck grades whether each facility server is still syncing with central, from central's point of view.
It is one of the doctor's healthchecks; see [CHK](healthchecks.md) for the framework it runs in and for how a check reports instances.

A facility server keeps working while it cannot reach central, so a stale sync is not itself an outage.
It is worth knowing about, and worth being able to set aside for a facility that is known to be offline without silencing the check for every other one.
The check therefore reports one instance per facility server, so each is graded and silenced by itself.

## What it measures

A facility server is identified by the device that runs it, as recorded on its sync sessions, and not by the facility it serves.
One device can serve more than one facility, and a facility's server can be replaced by a device with a different identity.

For each device that has had a sync session within the last two days, the check measures the minutes since the device's last successful sync.
A sync is successful when it completed without errors.
Mobile devices are not measured.
A session that names no device is not measured.

A device is active while it has a session within the last two days, so a decommissioned facility server stops being measured once it goes quiet.
The look-back for a last successful sync is bounded at thirty days.
A device with no successful sync inside it is graded as failed, with no reading for the time since.

A device's facilities are those named by its most recent session.
Their names are read from central's facility records, and a facility with no record is named by its id.

## Reporting

The check reports one instance per active device, keyed by the device's id.
Passing devices are reported as well as stale ones.

An instance's result is:

- passed when the last successful sync is no more than ten minutes ago.
- warning when it is more than ten and no more than thirty minutes ago.
- failed when it is more than thirty minutes ago, or when there is none inside the look-back.

An instance's label is the names of its device's facilities, joined with a comma.
Its detail carries the ids of those facilities, the time of the last successful sync and the minutes since it.

The check's own detail carries the warn and fail thresholds in minutes, which every instance shares.

A deployment with no active device reports an empty set of instances.

The check declares two gauges, the number of devices past the fail threshold and the number past the warn threshold ([MET](metrics.md)).

## Applicability

The check only applies to a central server.

- It skips on a facility server.
- It skips when the database cannot be reached, so that a database outage remains something the daemon can alert on rather than something that stops it.
- It reports broken when its query is faulty, and failed when its query fails for any other reason, in either case with no instances.
