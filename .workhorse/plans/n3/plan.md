# Self-update failure advice and `alertd logs`

When a delegated update fails, `bestool self-update` tells the operator to check the daemon logs with `bestool alertd status`.
That command never shows logs, and there is no command that does, so the operator has to find the log directory without being told where it is.
The daemon also keeps only the version that failed, not why, so the reason never leaves the log files.

## Decisions

- The self-update task records the failure reason alongside the failed version, in memory.
  Losing it on restart is fine: a failed update leaves the daemon running, and a successful one clears it by restarting.
- `/tasks/self-update/status` returns the reason next to `failed_version`.
  Adding a field is backward compatible; a new CLI talking to an old daemon gets no reason and falls back to a message naming only the version.
- `bestool self-update` puts the reason in its error and points at `bestool alertd logs` for more detail.
  The timeout message points at `bestool alertd logs` too, instead of the unspecified "daemon logs".
- `bestool alertd status` shows the last update failure (version and reason) when there is one, fetched from `/tasks/self-update/status`.
  The daemon's `/status` stays as it is, so the status endpoint doesn't depend on the self-update task.
- New `bestool alertd logs` command, on both platforms:
  - Windows: reads the service's JSON log files directly under `%ProgramData%\bestool\logs`, so it still works when the daemon is down or crash-looping.
  - Linux: shows the journal for the `bestool-alertd` identifier.
  - Prints entries as text on both platforms, with no JSON option.
- The Windows service writes its log files as text, not JSON, so the files and `alertd logs` read the same as the daemon's terminal output.
  - `-n`/`--lines` sets how many recent entries to show first, defaulting to 50.
  - Follows by default, whether or not stdout is a terminal; `--no-follow` prints and exits.

## Notes

- On Windows, lloggs starts a new file prefix at each process start (`bestool.<start-time>.log.<date>`, rotated daily).
  "The last N entries" therefore means merging across files by timestamp, and following has to pick up the new series a restart creates.
  The entries just before a restart are usually the interesting ones.
- lloggs's `setup` switches to its JSON formatter whenever `--log-file` is set, so writing text files needs either a lloggs option for the file format or bestool building that subscriber itself.
- `alertd install` prints "Logs are stored in JSON format with timestamps"; that line changes with the format.
- Hosts upgraded from a JSON-writing build keep JSON files until retention ages them out, so `alertd logs` will meet JSON lines in older files.
  Decide whether it renders them as text, prints them raw, or skips files it can't parse as text.

## Privileges

The spec requires the command to acquire the privileges it needs and to treat a still-unreadable log as an error, matching `bestool tamanu logs`.
On Linux that is `lifecycle::ensure_root_or_reexec`, as `tamanu logs` uses, since `journalctl` returns nothing and exits 0 for an unprivileged reader.
On Windows it is still unchecked whether a non-elevated user can read files LocalSystem writes under `%ProgramData%\bestool\logs`; check on a host, and if they can't, detect the access-denied and say to run elevated.
