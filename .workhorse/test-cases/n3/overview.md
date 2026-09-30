# Self-update failure reporting and alert daemon logs

## Update failures

- [x] The self-update status endpoint reports the failed version and its reason, flattened from the error chain (verifies spec: UPD)
- [x] With no failure recorded, the status endpoint reports neither a version nor a reason (verifies spec: UPD)
- [x] The CLI reads a failure with a reason, a failure from a daemon that sends no reason, and no failure (verifies spec: UPD)
- [x] `bestool alertd status` shows the failed version with its reason, or the version alone when the daemon sent no reason (verifies spec: UPD)
- [ ] On a Windows host, a delegated `bestool self-update` to a version that fails to install exits with an error naming the version and reason and pointing at `bestool alertd logs` (verifies spec: UPD)
- [ ] On a Windows host after a failed update, `bestool alertd status` shows the `Update:` line (verifies spec: UPD)
- [ ] On a Linux host, `bestool alertd status` shows no `Update:` line (verifies spec: UPD)

## Rendering

- [x] A JSON log entry renders as timestamp, padded level, target, message, and remaining fields (verifies spec: ALOG)
- [x] Spans render outermost first, with their fields (verifies spec: ALOG)
- [x] An event bridged from the `log` crate renders under its own target, without the `log.*` fields (verifies spec: ALOG)
- [x] A line that isn't a JSON entry doesn't render as one (verifies spec: ALOG)
- [x] A journal record renders with its timestamp in the daemon's form, including a message the journal hands over as bytes (verifies spec: ALOG)
- [x] Output stops without error when the reader goes away (verifies spec: ALOG)

## Windows log files

- [x] The most recent entries are merged across files by timestamp and limited to the requested count (verifies spec: ALOG)
- [x] A line still being written is left out of the recent entries and printed once it's complete (verifies spec: ALOG)
- [x] Following prints lines appended to known files and every line of a file that appears later (verifies spec: ALOG)
- [x] A deleted file is dropped from the follow (verifies spec: ALOG)
- [x] A line that isn't an entry stays after the entry it followed (verifies spec: ALOG)
- [x] A missing or empty log directory is an error naming the directory (verifies spec: ALOG)
- [x] A file that can't be read for lack of privileges is an error directing the operator to an elevated shell (verifies spec: ALOG)
- [x] The recent entries of a file larger than one read chunk are read from its end (verifies spec: ALOG)
- [ ] On a Windows host, `bestool alertd logs` from a non-elevated shell either shows the logs or directs the operator to an elevated shell (verifies spec: ALOG)
- [ ] On a Windows host, a follow keeps going across a `bestool alertd restart` and picks up the new file series (verifies spec: ALOG)

## Linux journal

- [ ] On a Linux host, `bestool alertd logs` as a non-root user re-executes under sudo and shows the journal entries (verifies spec: ALOG)
- [ ] On a Linux host with no `bestool-alertd` journal entries, the command exits with an error naming the identifier (verifies spec: ALOG)
- [ ] On a Linux host, `bestool alertd logs | grep WARN` keeps following (verifies spec: ALOG)
