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
  - Human-readable output by default, `--json` passes the stored lines through.
  - `-n`/`--lines` sets how many recent entries to show first, defaulting to 50.
  - Follows by default, whether or not stdout is a terminal; `--no-follow` prints and exits.

## Notes

- On Windows, lloggs starts a new file prefix at each process start (`bestool.<start-time>.log.<date>`, rotated daily).
  "The last N entries" therefore means merging across files by timestamp, and following has to pick up the new series a restart creates.
  The entries just before a restart are usually the interesting ones.

## Open questions

- Whether a non-elevated user can read `%ProgramData%\bestool\logs` given the files are written by LocalSystem; not yet checked on a host.
