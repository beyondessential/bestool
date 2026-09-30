---
id: ALOG
---

# Alert daemon logs

`bestool alertd logs` prints the alert daemon's recent log entries and then follows new ones as they are written.
It reads the logs from where the daemon's service writes them, so it works whether or not the daemon is running, including while it is crash-looping.
An operator runs the same command on every host regardless of platform.

## Log sources

On Windows the command reads the service's log files in `%ProgramData%\bestool\logs`.
The service writes these as JSON, one entry per line.
The daemon starts a new series of files each time it starts and rotates each series daily, and the command reads across every retained file, so entries from before a restart or a rotation are included.

On Linux the command reads the system journal entries recorded under the `bestool-alertd` identifier.

When no daemon logs exist at all, the command exits with an error naming where it looked.

Reading the logs requires privileges an operator may not hold.
The command acquires the privileges it needs before reading, and a log it still cannot read is an error, so an unreadable log cannot be mistaken for a quiet one.

## Output

Each entry is printed as text in the same form as the daemon's output when it runs in a terminal: its timestamp, level, and message, followed by its remaining fields.
On Windows the command renders each JSON entry into that form, so the output reads the same on every platform.
Entries are printed oldest first, in order of their timestamps, across all of the files they were read from.

## Options

The command first prints the most recent 50 entries, and `-n`/`--lines` sets a different count.

The command then follows, printing entries as they are written, until interrupted.
It follows whether or not its output is a terminal, so a follow piped into a filter keeps running.
Following continues across daily rotation, and across the new series of files the daemon starts when it restarts, so an update or a crash-and-restart does not end the follow.
`--no-follow` prints the recent entries and exits.
