# A1: --only-failing + TUI for `bestool tamanu doctor`

Implements spec `DOC` (see `.workhorse/specs/tamanu/doctor.md`). Two coupled changes:

1. Add `--only-failing` / `-F` flag that filters out pass/skip outcomes.
2. Replace the line-rewriting live render with a ratatui alternate-screen TUI; reorder all output (live, replay, non-TTY) into the new severity-grouped layout.

## Design notes

### Ordering

A single sort key per row:

- group_order: pending=0, running=1, pass=2, skip=3, warning=4, broken=5, fail=6
- within group: alphabetical by check name

Final rendered output (and non-TTY output): same key, but pending/running can't appear (sweep has finished). Under `-F`, rows with status pass or skip are hidden.

This keeps the "least severe at top, most severe at bottom" theme: the most urgent rows sit nearest the result line (and shell prompt) where the eye lands.

### TUI

Built on ratatui + crossterm in alternate-screen mode. The check list takes the bulk of the screen, with a footer line showing `<spinner> <completed> / <total> complete`.

Each row tracks its own state (Pending → Running → Completed(Check)) and renders accordingly:

- Pending: dim `····` indicator + check name
- Running: spinner glyph (animated) + check name
- Completed: coloured outcome tag (PASS/SKIP/WARN/BRKN/FAIL) + check name + summary; reason as a dim continuation line for non-pass outcomes

A dimmed source note sits below the result line in the final render. In the TUI, it appears at the top of the screen (dimmed) so the operator sees it while the sweep is running.

Tick at ~10Hz to drive spinner animation and drain incoming progress events from the existing mpsc channel.

On completion (or Ctrl+C), restore the terminal and replay the same rendered output to stdout via the plain renderer so it persists in scrollback.

### Filter under -F

The TUI always shows pending/running rows for every selected check (progress would be invisible otherwise). When a check completes:

- If the outcome would survive the filter (warning/broken/fail under -F, or anything without -F), the row moves to its grouped-and-ordered position.
- If the outcome is hidden by the filter (pass/skip under -F), the row is removed from the displayed list.

Footer count tracks completion against the full selected set, not the filtered displayed list.

### Server-id removed

The spec drops the `Tamanu doctor (server-id: …)` header line from human-readable output. Server-id still flows through the JSON wire payload — only the human render changes.

### Dependencies

Add `ratatui` to bestool's `tamanu-doctor` feature. Pair with `crossterm` (already a transitive via psql but added explicitly here for terminal lifecycle handling — Ctrl+C raw mode, alt-screen entry/exit).

### Module split

`crates/bestool/src/actions/tamanu/doctor.rs` grows enough to warrant splitting:

- `doctor.rs` — `DoctorArgs`, `run()`, orchestration
- `doctor/order.rs` — group ordering / sort helpers
- `doctor/render.rs` — plain text rendering for non-TTY and replay
- `doctor/tui.rs` — ratatui live TUI

## Checklist

- [x] Add `ratatui` and `crossterm` deps to bestool (`tamanu-doctor` feature)
- [x] Add `DoctorArgs::only_failing` field with `--only-failing` / `-F`
- [x] Add ordering module (group key + sort + filter helper)
- [x] Add plain render module (grouped, with -F support, source note, no server-id header)
- [x] Add TUI module with row-state model, spinner, footer count, source note
- [x] Hook Ctrl+C into TUI teardown so partial results still replay to scrollback
- [x] Wire `run()` to choose TUI vs plain based on TTY + JSON
- [x] Update JSON path to ignore `--only-failing`
- [x] Remove old `render_live`, `render`, `write_check_line`, `truncate_to_width`, etc.
- [x] Refresh tests: drop server-id assertions, add grouped-ordering tests, add only-failing tests
- [ ] Run `cargo clippy` and `cargo fmt` (needs external cargo run)
