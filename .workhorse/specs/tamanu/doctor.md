---
id: DOC
---

# Tamanu doctor output

The `bestool tamanu doctor` command runs a set of health checks against a Tamanu install and renders the outcome. Each check resolves to one of five outcomes: pass, skip, warning, broken (the check itself errored), or fail. This spec describes how the command displays results during the sweep, in the final rendered output, and when the only-failing filter is applied.

## Grouping and ordering

- [ ] Checks are grouped by outcome
- [ ] Groups appear top-to-bottom in increasing severity: passing, skipped, warning, broken, failing — so the most urgent checks sit closest to the result line and the shell prompt
- [ ] Within each group, checks are ordered alphabetically by name
- [ ] The same grouping and ordering apply during the live sweep and in the final rendered output

## Live execution in an interactive terminal

- [ ] When stdout is attached to a terminal and JSON output is not requested, the command takes over the terminal in an alternate-screen TUI for the duration of the sweep
- [ ] Every selected check appears as a row in the list from the start of the sweep, in a pending state
- [ ] A pending check shows a neutral indicator, a running check shows an animated spinner, and a completed check shows its outcome tag alongside the check name and one-line summary
- [ ] As each check completes, its row moves to its grouped-and-ordered position
- [ ] A footer line shows an animated spinner together with a running count of how many checks have completed out of the total
- [ ] A dimmed source note in the TUI identifies whether the data is being computed locally, streamed from the alertd daemon, or read from the daemon's last cached sweep
- [ ] On completion, the TUI tears down and the final rendered output is written to the terminal's normal scrollback so it remains visible after the command exits
- [ ] If the user interrupts the command (Ctrl+C), the TUI tears down cleanly and whatever results have been collected so far are written to scrollback before exit

## Final rendered output

- [ ] The final output lists each displayed check on its own row with a coloured outcome tag, the check name, and a one-line summary, in grouped-and-ordered position
- [ ] A blank line separates the check list from the result line
- [ ] The result line gives an overall outcome — healthy, degraded, or failing — and counts of failed, warning, broken, and skipped checks
- [ ] A dimmed source note states whether the data was computed locally, streamed from the alertd daemon, or read from the daemon's last cached sweep, including how long ago a cached sweep was computed
- [ ] The result line is always shown, even when the displayed check list is empty

## Only-failing filter

- [ ] The command accepts `--only-failing` and its short form `-F`
- [ ] With the filter set, only checks with warning, broken, or failing outcomes appear in the displayed list and the final output
- [ ] The grouping and ordering rules still apply, so displayed checks are ordered warning, then broken, then failing
- [ ] In the live TUI under the filter, pending and running rows remain visible for every selected check; once a check's outcome is known, it stays in the list only if it warned, broke, or failed
- [ ] The footer count under the filter still tracks completion against the full set of selected checks
- [ ] When every selected check passes or is skipped, the displayed list is empty and the result line is shown on its own

## Non-interactive output

- [ ] When stdout is not attached to a terminal and JSON output is not requested, the command produces line-by-line output without an alternate-screen TUI or animated spinner
- [ ] Non-interactive output follows the same grouping, ordering, source note, result line, and only-failing filter rules

## JSON output

- [ ] When JSON output is requested, the command emits the full machine-readable sweep payload and suppresses the human-readable rendering
- [ ] The only-failing filter does not affect JSON output: every selected check is included

## Exit code

- [ ] The command exits non-zero when the overall outcome is failing, and zero otherwise
- [ ] An exit caused by user interruption is non-zero regardless of partial results
