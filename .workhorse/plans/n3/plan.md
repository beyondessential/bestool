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
  - On Windows it renders the service's JSON log lines into the terminal format; on Linux the journal is already text.
  - `-n`/`--lines` sets how many recent entries to show first, defaulting to 50.
  - Follows by default, whether or not stdout is a terminal; `--no-follow` prints and exits.
- The Windows service keeps writing JSON log files.
  JSON escapes newlines, so every entry is one line, which keeps counting the last N entries and merging files by timestamp simple.
  Switching to text would have needed a lloggs change (its `setup` always uses JSON with `--log-file`) or a bestool-owned subscriber, a transition for old JSON files, and grouping of multi-line entries.

## Notes

- On Windows, lloggs starts a new file prefix at each process start (`bestool.<start-time>.log.<date>`, rotated daily).
  "The last N entries" therefore means merging across files by timestamp, and following has to pick up the new series a restart creates.
  The entries just before a restart are usually the interesting ones.
- The renderer handles tracing's JSON shape: `timestamp`, `level`, `target`, `fields` (with `message`), and `span`/`spans` when present.
  A line that doesn't parse is printed as it is, so a partial line at the end of a file being written doesn't break the output.

## Privileges

The spec requires the command to acquire the privileges it needs and to treat a still-unreadable log as an error, matching `bestool tamanu logs`.
On Linux that is a sudo re-exec, as `tamanu logs` does with `lifecycle::ensure_root_or_reexec`, since `journalctl` returns nothing and exits 0 for an unprivileged reader.
On Windows the command can't elevate in place (runas opens a separate console), so an access-denied read is an error directing the operator to an elevated shell, the same wording as the rdp commands use.
Whether a non-elevated user hits that on a real host is still unchecked.

## Build steps

- [x] Self-update task records `{version, reason}`; `/tasks/self-update/status` returns `failed_reason` alongside `failed_version`
- [x] `bestool self-update` error carries the reason and points at `bestool alertd logs`; timeout message names `bestool alertd status` and `bestool alertd logs`
- [x] `bestool alertd status` shows `Update:` with the failed version and reason when the daemon reports one (a 404 from a daemon without the task means no line)
- [x] `bestool alertd logs` subcommand (`-n`/`--lines` default 50, `--no-follow`)
  - [x] Render tracing JSON lines into the terminal form (timestamp, padded level, spans, target, message, fields; `log.*` fields folded as the terminal formatter does)
  - [x] Windows: last N entries across every file in the service log dir, merged by timestamp; unparseable lines keep their place after the preceding entry
  - [x] Windows: follow by polling the dir, picking up new files (daily rotation, new series on restart) from their start and dropping deleted ones
  - [x] Linux: `journalctl -t bestool-alertd --output=json`, rendered as timestamp + message; re-exec under sudo when not root; error when there are no entries
  - [x] Errors: no log dir or no files names the path; unreadable file is an error
- [x] `alertd install` output mentions `bestool alertd logs`
- [x] Tests for the renderer, merge/last-N, follow pickup of new files, journal record rendering, failure-status parsing
- [x] `cargo clippy`, `cargo fmt`, `cargo check --target x86_64-pc-windows-gnu`
  - The Windows check ran with `--no-default-features --features alertd,self-update`: the default feature set fails here in `turso_sdk_kit`'s build script (pulled in by `bestool-psql`), which can't compile its Windows version resource on this machine
