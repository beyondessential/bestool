use std::{
	io::{self, Stdout, Write},
	time::Duration,
};

use crossterm::{
	cursor::{Hide, MoveTo, Show},
	event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
	execute, queue,
	style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor},
	terminal::{
		Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
		enable_raw_mode, size,
	},
};
use miette::{IntoDiagnostic, Result};
use tokio::sync::mpsc::UnboundedReceiver;

use bestool_alertd::doctor::{
	check::{Check, CheckStatus},
	progress::DoctorEvent,
};

use super::{SweepSource, order};

const SPINNER_FRAMES: &[&str] = &[
	"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏",
];
const TICK: Duration = Duration::from_millis(80);

/// State of a single row in the TUI.
#[derive(Clone)]
enum RowState {
	Running,
	Completed(Check),
}

struct TuiRow {
	name: &'static str,
	state: RowState,
}

pub struct TuiOutcome {
	pub results: Vec<(Check, bool)>,
	pub interrupted: bool,
}

/// Vertical scroll position for the row list when it is taller than the
/// terminal. `offset` is the number of body lines hidden above the viewport;
/// `viewport` and `max` are the dimensions from the last draw, used to clamp
/// keyboard scrolling.
#[derive(Default)]
struct Scroll {
	offset: usize,
	viewport: usize,
	max: usize,
}

impl Scroll {
	fn by(&mut self, delta: i64) {
		let next = (self.offset as i64).saturating_add(delta).clamp(0, self.max as i64);
		self.offset = next as usize;
	}
}

/// A single styled segment within a rendered line.
#[derive(Clone, Debug)]
struct Segment {
	text: String,
	style: SegStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegStyle {
	Plain,
	Dim,
	BoldFg(Color),
}

type StyledLine = Vec<Segment>;

fn plain<S: Into<String>>(s: S) -> Segment {
	Segment {
		text: s.into(),
		style: SegStyle::Plain,
	}
}
fn dim<S: Into<String>>(s: S) -> Segment {
	Segment {
		text: s.into(),
		style: SegStyle::Dim,
	}
}
fn bold_fg<S: Into<String>>(s: S, c: Color) -> Segment {
	Segment {
		text: s.into(),
		style: SegStyle::BoldFg(c),
	}
}

/// RAII guard that restores the terminal on drop (including on panic).
struct TerminalGuard {
	stdout: Stdout,
}

impl TerminalGuard {
	fn new() -> io::Result<Self> {
		let mut stdout = io::stdout();
		enable_raw_mode()?;
		execute!(stdout, EnterAlternateScreen, Hide)?;
		Ok(Self { stdout })
	}
}

impl Drop for TerminalGuard {
	fn drop(&mut self) {
		let _ = execute!(self.stdout, Show, LeaveAlternateScreen);
		let _ = disable_raw_mode();
	}
}

/// Run the live TUI until either the sweep finishes (progress channel closes
/// and every selected check has been seen) or the user interrupts (Ctrl+C / q).
///
/// This is synchronous and must be run via [`tokio::task::spawn_blocking`], not
/// `tokio::spawn`: its loop has no `.await` points (the terminal I/O and
/// `crossterm::event::poll` are plain blocking calls with no runtime
/// integration), so as an async task it would never yield back to the
/// executor. On a single-worker-thread runtime — the default on a
/// single-vCPU host — that starves every other task (the sweep itself
/// included) for as long as the TUI keeps running, which reads as a total,
/// permanent hang. `spawn_blocking` moves it onto Tokio's separate blocking
/// thread pool, which is sized independently of the CPU count, so it can
/// never contend with the async worker pool.
pub fn run_tui(
	selected_names: Vec<&'static str>,
	source: SweepSource,
	mut progress_rx: UnboundedReceiver<DoctorEvent>,
) -> Result<TuiOutcome> {
	let total = selected_names.len();
	let mut rows: Vec<TuiRow> = selected_names
		.into_iter()
		.map(|name| TuiRow {
			name,
			state: RowState::Running,
		})
		.collect();
	let mut spinner = 0usize;
	let interrupted;
	let mut scroll = Scroll::default();

	let mut guard = TerminalGuard::new().into_diagnostic()?;

	loop {
		drain_progress(&mut progress_rx, &mut rows);
		let finalising = all_completed(&rows);

		draw(
			&mut guard.stdout,
			&rows,
			total,
			&source,
			spinner,
			finalising,
			&mut scroll,
		)
		.into_diagnostic()?;

		// The sweep keeps working after the last check reports (gathering server
		// facts and probing connectivity); its progress sender drops only when it
		// fully returns. Stay up until the channel closes so the replay prints the
		// instant the terminal is restored, rather than after a visible gap.
		if progress_rx.is_closed() && progress_rx.is_empty() {
			// All checks done means a clean finish; anything less means the sweep
			// ended early (error or abort) and the caller should exit non-zero.
			interrupted = !finalising;
			break;
		}

		if poll_input(TICK, &mut scroll).into_diagnostic()? {
			interrupted = true;
			break;
		}

		spinner = (spinner + 1) % SPINNER_FRAMES.len();
	}

	drop(guard);

	let results: Vec<(Check, bool)> = rows
		.into_iter()
		.filter_map(|row| match row.state {
			RowState::Completed(check) => Some((check, true)),
			RowState::Running => None,
		})
		.collect();

	Ok(TuiOutcome {
		results,
		interrupted,
	})
}

fn drain_progress(rx: &mut UnboundedReceiver<DoctorEvent>, rows: &mut [TuiRow]) {
	while let Ok(evt) = rx.try_recv() {
		match evt {
			DoctorEvent::Completed(check) => {
				if let Some(row) = rows.iter_mut().find(|r| r.name == check.name) {
					row.state = RowState::Completed(check);
				}
			}
		}
	}
}

fn all_completed(rows: &[TuiRow]) -> bool {
	rows.iter().all(|r| matches!(r.state, RowState::Completed(_)))
}

/// Poll keyboard events for up to `timeout`, applying scroll keys to `scroll`.
/// Returns true if the user pressed a quit chord (Ctrl+C or q).
fn poll_input(timeout: Duration, scroll: &mut Scroll) -> io::Result<bool> {
	if !event::poll(timeout)? {
		return Ok(false);
	}
	while event::poll(Duration::ZERO)? {
		if let Event::Key(KeyEvent {
			code, modifiers, ..
		}) = event::read()?
		{
			let ctrl_c =
				matches!(code, KeyCode::Char('c')) && modifiers.contains(KeyModifiers::CONTROL);
			if ctrl_c || matches!(code, KeyCode::Char('q')) {
				return Ok(true);
			}
			let page = scroll.viewport.max(1) as i64;
			match code {
				KeyCode::Up | KeyCode::Char('k') => scroll.by(-1),
				KeyCode::Down | KeyCode::Char('j') => scroll.by(1),
				KeyCode::PageUp => scroll.by(-page),
				KeyCode::PageDown | KeyCode::Char(' ') => scroll.by(page),
				KeyCode::Home | KeyCode::Char('g') => scroll.by(i64::MIN),
				KeyCode::End | KeyCode::Char('G') => scroll.by(i64::MAX),
				_ => {}
			}
		}
	}
	Ok(false)
}

fn draw(
	out: &mut Stdout,
	rows: &[TuiRow],
	total: usize,
	source: &SweepSource,
	spinner: usize,
	finalising: bool,
	scroll: &mut Scroll,
) -> io::Result<()> {
	let term_rows = size().map(|(_, r)| r as usize).unwrap_or(24).max(2);

	let body = build_rows(rows, spinner);

	// The header (where the sweep is running) is pinned to the top row and the
	// footer (progress) to the bottom row; the body region is what's between.
	let viewport = term_rows.saturating_sub(2);
	scroll.viewport = viewport;
	scroll.max = body.len().saturating_sub(viewport);
	scroll.offset = scroll.offset.min(scroll.max);

	let end = (scroll.offset + viewport).min(body.len());
	let visible = &body[scroll.offset..end];

	// Redraw in place rather than clearing the whole screen first: clearing each
	// row to end-of-line avoids the flash a full clear produces every tick. The
	// body is anchored to the bottom of its region (blank rows padded above it),
	// so a short check list keeps the content and footer together at the bottom
	// instead of stranding the footer under a block of whitespace.
	queue!(out, MoveTo(0, 0))?;
	write_line(out, &header_line(source))?;
	queue!(out, Clear(ClearType::UntilNewLine))?;
	out.write_all(b"\r\n")?;
	for _ in 0..viewport.saturating_sub(visible.len()) {
		queue!(out, Clear(ClearType::UntilNewLine))?;
		out.write_all(b"\r\n")?;
	}
	for line in visible {
		write_line(out, line)?;
		queue!(out, Clear(ClearType::UntilNewLine))?;
		out.write_all(b"\r\n")?;
	}

	write_line(out, &footer_line(rows, total, spinner, finalising, scroll))?;
	queue!(out, Clear(ClearType::UntilNewLine))?;
	out.flush()
}

fn write_line(out: &mut Stdout, line: &StyledLine) -> io::Result<()> {
	for seg in line {
		match seg.style {
			SegStyle::Plain => out.write_all(seg.text.as_bytes())?,
			SegStyle::Dim => {
				queue!(out, SetAttribute(Attribute::Dim))?;
				out.write_all(seg.text.as_bytes())?;
				queue!(out, SetAttribute(Attribute::Reset))?;
			}
			SegStyle::BoldFg(c) => {
				queue!(out, SetForegroundColor(c), SetAttribute(Attribute::Bold))?;
				out.write_all(seg.text.as_bytes())?;
				queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
			}
		}
	}
	Ok(())
}

/// The fixed top row: where this sweep is running.
fn header_line(source: &SweepSource) -> StyledLine {
	let text = match source {
		SweepSource::Local => "Source: local".to_string(),
		SweepSource::DaemonStreamed => "Source: alertd daemon (just now, on demand)".to_string(),
		SweepSource::DaemonCached { computed_at } => {
			let age = super::render::humanise_age_since(*computed_at);
			format!("Source: alertd daemon (computed {age} ago, at {computed_at})")
		}
	};
	vec![dim(text)]
}

fn build_rows(rows: &[TuiRow], spinner: usize) -> Vec<StyledLine> {
	let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);

	// Skipped checks drop out of the live list once their outcome is known;
	// pending and running rows stay until they resolve.
	let mut ordered: Vec<&TuiRow> = rows
		.iter()
		.filter(|row| match &row.state {
			RowState::Running => true,
			RowState::Completed(check) => !matches!(check.status, CheckStatus::Skip(_)),
		})
		.collect();
	ordered.sort_by(|a, b| sort_key(a).cmp(&sort_key(b)).then_with(|| a.name.cmp(b.name)));

	let mut lines = Vec::with_capacity(ordered.len());
	for row in ordered {
		match &row.state {
			RowState::Running => lines.push(row_line_running(row.name, name_width, spinner)),
			RowState::Completed(check) => {
				lines.push(row_line_completed(check, name_width));
				if let Some(reason) = reason_for(check) {
					lines.push(reason_line(reason, name_width));
				}
			}
		}
	}
	lines
}

fn sort_key(row: &TuiRow) -> u8 {
	match &row.state {
		RowState::Running => 0,
		RowState::Completed(check) => 1 + order::severity_key(&check.status),
	}
}

fn row_line_running(name: &str, name_width: usize, spinner: usize) -> StyledLine {
	let frame = SPINNER_FRAMES[spinner % SPINNER_FRAMES.len()];
	let pad = " ".repeat(name_width.saturating_sub(name.len()));
	vec![
		plain("  "),
		dim(format!("{frame:<4}")),
		plain("    "),
		plain(name.to_string()),
		plain(pad),
		plain("   "),
		dim("…"),
	]
}

fn row_line_completed(check: &Check, name_width: usize) -> StyledLine {
	let (tag, color) = tag_for(check);
	let pad = " ".repeat(name_width.saturating_sub(check.name.len()));
	vec![
		plain("  "),
		bold_fg(format!("{tag:<4}"), color),
		plain("    "),
		plain(check.name.to_string()),
		plain(pad),
		plain("   "),
		plain(check.summary.clone()),
	]
}

fn reason_line(reason: &str, name_width: usize) -> StyledLine {
	let lead = " ".repeat(10 + name_width + 5);
	vec![plain(lead), dim(reason.to_string())]
}

fn reason_for(check: &Check) -> Option<&str> {
	match &check.status {
		CheckStatus::Pass => None,
		CheckStatus::Skip(r)
		| CheckStatus::Warning(r)
		| CheckStatus::Fail(r)
		| CheckStatus::Broken(r) => Some(r.as_str()),
	}
}

fn tag_for(check: &Check) -> (&'static str, Color) {
	match &check.status {
		CheckStatus::Pass => ("PASS", Color::Green),
		CheckStatus::Skip(_) => ("SKIP", Color::DarkGrey),
		CheckStatus::Warning(_) => ("WARN", Color::Yellow),
		CheckStatus::Broken(_) => ("BRKN", Color::Magenta),
		CheckStatus::Fail(_) => ("FAIL", Color::Red),
	}
}

fn footer_line(
	rows: &[TuiRow],
	total: usize,
	spinner: usize,
	finalising: bool,
	scroll: &Scroll,
) -> StyledLine {
	let completed = rows
		.iter()
		.filter(|r| matches!(r.state, RowState::Completed(_)))
		.count();
	let frame = SPINNER_FRAMES[spinner % SPINNER_FRAMES.len()];
	let status = if finalising {
		format!("{frame} finalising… ({completed} / {total} checks done)")
	} else {
		format!("{frame} {completed} / {total} complete")
	};
	let mut line = vec![dim(status)];
	if scroll.max > 0 {
		let above = scroll.offset;
		let below = scroll.max - scroll.offset;
		line.push(dim(format!(
			"   ↑/↓ scroll ({above} above, {below} below)"
		)));
	}
	line
}

#[cfg(test)]
mod tests {
	use super::*;

	fn line_text(line: &StyledLine) -> String {
		line.iter().map(|s| s.text.as_str()).collect()
	}

	#[test]
	fn build_rows_orders_running_then_pass_warn_broken_fail() {
		let rows = vec![
			TuiRow {
				name: "z-fail",
				state: RowState::Completed(Check::fail("z-fail", "bad", "r")),
			},
			TuiRow {
				name: "a-pass",
				state: RowState::Completed(Check::pass("a-pass", "ok")),
			},
			TuiRow {
				name: "m-warn",
				state: RowState::Completed(Check::warning("m-warn", "deg", "r")),
			},
			TuiRow {
				name: "k-run",
				state: RowState::Running,
			},
			TuiRow {
				name: "c-broken",
				state: RowState::Completed(Check::broken("c-broken", "broke", "r")),
			},
		];
		let lines = build_rows(&rows, 0);
		let joined: Vec<String> = lines.iter().map(line_text).collect();
		let positions = |needle: &str| {
			joined
				.iter()
				.position(|s| s.contains(needle))
				.unwrap_or_else(|| panic!("missing {needle}"))
		};
		assert!(positions("k-run") < positions("a-pass"));
		assert!(positions("a-pass") < positions("m-warn"));
		assert!(positions("m-warn") < positions("c-broken"));
		assert!(positions("c-broken") < positions("z-fail"));
	}

	#[test]
	fn build_rows_drops_completed_skip_but_keeps_pass_and_running() {
		let rows = vec![
			TuiRow {
				name: "a-pass",
				state: RowState::Completed(Check::pass("a-pass", "ok")),
			},
			TuiRow {
				name: "b-skip",
				state: RowState::Completed(Check::skip("b-skip", "n/a", "r")),
			},
			TuiRow {
				name: "c-warn",
				state: RowState::Completed(Check::warning("c-warn", "deg", "r")),
			},
			TuiRow {
				name: "d-run",
				state: RowState::Running,
			},
		];
		let lines = build_rows(&rows, 0);
		let joined: Vec<String> = lines.iter().map(line_text).collect();
		assert!(joined.iter().any(|s| s.contains("d-run")));
		assert!(joined.iter().any(|s| s.contains("c-warn")));
		assert!(joined.iter().any(|s| s.contains("a-pass")));
		assert!(!joined.iter().any(|s| s.contains("b-skip")));
	}

	#[test]
	fn footer_counts_completed_against_total() {
		let rows = vec![
			TuiRow {
				name: "a",
				state: RowState::Completed(Check::pass("a", "ok")),
			},
			TuiRow {
				name: "b",
				state: RowState::Completed(Check::warning("b", "deg", "r")),
			},
			TuiRow {
				name: "c",
				state: RowState::Running,
			},
		];
		let line = footer_line(&rows, 3, 0, false, &Scroll::default());
		assert!(line_text(&line).contains("2 / 3 complete"));
	}

	#[test]
	fn footer_shows_finalising_when_all_done() {
		let rows = vec![TuiRow {
			name: "a",
			state: RowState::Completed(Check::pass("a", "ok")),
		}];
		let line = footer_line(&rows, 1, 0, true, &Scroll::default());
		assert!(line_text(&line).contains("finalising"));
	}

	#[test]
	fn footer_shows_scroll_hint_only_when_scrollable() {
		let rows = vec![TuiRow {
			name: "a",
			state: RowState::Running,
		}];
		let none = footer_line(&rows, 1, 0, false, &Scroll::default());
		assert!(!line_text(&none).contains("scroll"));

		let scroll = Scroll {
			offset: 2,
			viewport: 5,
			max: 7,
		};
		let some = footer_line(&rows, 1, 0, false, &scroll);
		let text = line_text(&some);
		assert!(text.contains("2 above"));
		assert!(text.contains("5 below"));
	}

	#[test]
	fn scroll_by_clamps_to_range() {
		let mut s = Scroll {
			offset: 0,
			viewport: 5,
			max: 4,
		};
		s.by(-1);
		assert_eq!(s.offset, 0);
		s.by(2);
		assert_eq!(s.offset, 2);
		s.by(100);
		assert_eq!(s.offset, 4);
		s.by(i64::MIN);
		assert_eq!(s.offset, 0);
	}
}
