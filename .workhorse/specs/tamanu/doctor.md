---
id: DOC
---

# Tamanu doctor

The `bestool tamanu doctor` command gathers server facts and runs a set of health checks against a Tamanu install, then renders the outcome for a human operator or as a machine-readable payload.
Checks may pass, be skipped, warn, fail, or be broken (the check itself errored).
This spec describes how the command selects checks, where the sweep data comes from, and how results are displayed.

## Checks and outcomes

The command runs a fixed registry of named checks covering both host-level concerns (disk, memory, time sync, and so on) and Tamanu-specific concerns (database, HTTP, services, certificates, sync state).
Each check resolves to exactly one of five outcomes: pass, skip, warning, broken, or fail.
Every check produces a one-line summary; checks with a skip, warning, broken, or fail outcome also carry a reason.

The overall outcome of the sweep is failing if any check failed, degraded if any check warned or broke without any failing, and healthy otherwise.
Skipped checks do not degrade the overall outcome.

## Selecting checks

`--check NAME` restricts the sweep to the named check and is repeatable to select several.
`--skip NAME` excludes the named check, is repeatable, and applies after `--check`.
With no selection flags, every check in the registry runs.
An unknown check name in either flag is a fatal error that lists the known check names.

## Tamanu install context

When a Tamanu install is present on the host, the sweep runs against it: install-dependent checks (configuration, local HTTP, services, certificates, backup) and database-dependent checks both run.
When no install is present but a Tamanu database URL is configured via the environment, the sweep runs against that database — database-dependent checks run and install-dependent checks skip.
When neither is present, only host-level checks run and the command warns that no Tamanu was found on this host.

## Sweep source

By default, the command reads the most recent sweep cached by the alertd daemon on the same host.
`--fresh` asks the daemon to recompute and streams the per-check results back as they complete.
`--no-daemon` skips the daemon integration and computes the sweep locally.
If the daemon cannot be reached or returns an error, the command falls back to computing the sweep locally.

A source note in both the live display and the final rendered output identifies whether the data was computed locally, streamed from the daemon, or read from the daemon's last cached sweep, and how long ago a cached sweep was computed.

## Grouping and ordering

Checks are grouped by outcome.
Groups appear top-to-bottom in increasing severity: passing, skipped, warning, broken, failing — so the most urgent checks sit closest to the result line and the shell prompt.
Within each group, checks are ordered alphabetically by name.
The same grouping and ordering apply during the live sweep and in the final rendered output.

## Live execution in an interactive terminal

When stdout is attached to a terminal that honours ANSI escape sequences and JSON output is not requested, the command takes over the terminal in an alternate-screen TUI for the duration of the sweep.
A terminal that cannot render ANSI escapes (for instance an older console that does not support them) is treated as non-interactive, and styled colour output is likewise suppressed there.
Every selected check appears as a row in the list from the start of the sweep, in a pending state.
A pending check shows a neutral indicator, a running check shows an animated spinner, and a completed check shows its outcome tag alongside the check name and one-line summary.
As each check completes, its row moves to its grouped-and-ordered position; a check that completes as skipped is removed from the live list entirely.

When the list of rows is taller than the terminal, the operator can scroll it from the keyboard, and the footer indicates how many rows are hidden above and below the viewport.

A footer line shows an animated spinner together with a running count of how many checks have completed out of the total.
The source note appears in the TUI as dimmed text.

The sweep continues gathering server facts after the last check completes; the live display stays up and the footer indicates that the sweep is finalising until the sweep fully returns, so the final output appears immediately when the live display tears down rather than after a gap.

On completion, the TUI tears down and the final rendered output is written to the terminal's normal scrollback so it remains visible after the command exits.
If the user interrupts the command (Ctrl+C), the TUI tears down cleanly and whatever results have been collected so far are written to scrollback before exit.

## Final rendered output

The final output lists each displayed check on its own row with a coloured outcome tag, the check name, and a one-line summary, in grouped-and-ordered position.
A blank line separates the check list from the result line.
The result line gives an overall outcome — healthy, degraded, or failing — and counts of passed, failed, warning, broken, and skipped checks across the whole sweep, regardless of which checks the replay filter displays.
The source note appears as dimmed text below the result line.
The result line is always shown, even when the displayed check list is empty.

## Result replay filter

By default the final rendered output lists only checks with warning, broken, or failing outcomes; passing and skipped checks are omitted.
`--all` and its short form `-a` instead include every selected check in the output.
The grouping and ordering rules still apply, so without `--all` the displayed checks are ordered warning, then broken, then failing.

The filter applies only to the final rendered output, never to the live TUI: passing checks stay visible in the live display even though they are omitted from the default output (skipped checks drop from the live display as described above).
When no check warns, breaks, or fails and `--all` is not set, the displayed list is empty and the result line is shown on its own.

## Non-interactive output

When stdout is not attached to an ANSI-capable terminal and JSON output is not requested, the command produces line-by-line output without an alternate-screen TUI or animated spinner.
Non-interactive output follows the same grouping, ordering, source note, result line, and replay filter rules.

## JSON output

When JSON output is requested, the command emits a machine-readable payload and suppresses the human-readable rendering.
The payload contains the sweep's wire data, a source field naming where the data came from, and a computed-at timestamp when the source is the daemon's last cached sweep.
The replay filter does not affect JSON output: every selected check is included.

## Exit code

The command exits non-zero when the overall outcome is failing, and zero otherwise.
An exit caused by user interruption is non-zero regardless of partial results.
