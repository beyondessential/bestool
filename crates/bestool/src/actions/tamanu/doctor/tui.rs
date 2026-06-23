use std::{
	io::{self, Stdout},
	time::Duration,
};

use miette::{IntoDiagnostic, Result};
use ratatui::{
	Terminal,
	backend::CrosstermBackend,
	crossterm::{
		event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
		execute,
		terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
	},
	layout::{Constraint, Direction, Layout},
	style::{Color, Modifier, Style},
	text::{Line, Span},
	widgets::Paragraph,
};
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

/// RAII guard that restores the terminal on drop (including on panic).
struct TerminalGuard {
	terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
	fn new() -> io::Result<Self> {
		let mut stdout = io::stdout();
		enable_raw_mode()?;
		execute!(stdout, EnterAlternateScreen)?;
		let backend = CrosstermBackend::new(stdout);
		let terminal = Terminal::new(backend)?;
		Ok(Self { terminal })
	}
}

impl Drop for TerminalGuard {
	fn drop(&mut self) {
		let _ = disable_raw_mode();
		let _ = execute!(io::stdout(), LeaveAlternateScreen);
		let _ = self.terminal.show_cursor();
	}
}

/// Run the live TUI until either the sweep finishes (progress channel closes
/// and every selected check has been seen) or the user interrupts (Ctrl+C / q).
pub async fn run_tui(
	selected_names: Vec<&'static str>,
	only_failing: bool,
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
	let mut interrupted = false;

	let mut guard = TerminalGuard::new().into_diagnostic()?;
	guard.terminal.hide_cursor().into_diagnostic()?;

	loop {
		drain_progress(&mut progress_rx, &mut rows);

		guard
			.terminal
			.draw(|f| draw(f, &rows, total, &source, only_failing, spinner))
			.into_diagnostic()?;

		if all_completed(&rows) {
			break;
		}

		if poll_for_quit(TICK).into_diagnostic()? {
			interrupted = true;
			break;
		}

		spinner = (spinner + 1) % SPINNER_FRAMES.len();

		if progress_rx.is_closed() && progress_rx.is_empty() && !all_completed(&rows) {
			// Sweep ended early (error or aborted) without producing all results.
			// Treat as interrupted so the caller exits non-zero.
			interrupted = true;
			break;
		}
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

/// Poll keyboard events for up to `timeout`. Returns true if the user pressed a
/// quit chord (Ctrl+C or q).
fn poll_for_quit(timeout: Duration) -> io::Result<bool> {
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
			let q = matches!(code, KeyCode::Char('q'));
			if ctrl_c || q {
				return Ok(true);
			}
		}
	}
	Ok(false)
}

fn draw(
	f: &mut ratatui::Frame<'_>,
	rows: &[TuiRow],
	total: usize,
	source: &SweepSource,
	only_failing: bool,
	spinner: usize,
) {
	let area = f.area();
	let layout = Layout::default()
		.direction(Direction::Vertical)
		.constraints([
			Constraint::Length(source_lines(source)),
			Constraint::Min(0),
			Constraint::Length(1),
		])
		.split(area);

	let source_para = Paragraph::new(source_line(source)).style(dim_style());
	f.render_widget(source_para, layout[0]);

	let lines = build_rows(rows, only_failing, spinner);
	let list_para = Paragraph::new(lines);
	f.render_widget(list_para, layout[1]);

	let footer = footer_line(rows, total, spinner);
	let footer_para = Paragraph::new(footer).style(dim_style());
	f.render_widget(footer_para, layout[2]);
}

fn source_lines(source: &SweepSource) -> u16 {
	match source {
		SweepSource::Local => 0,
		_ => 1,
	}
}

fn source_line(source: &SweepSource) -> Line<'_> {
	match source {
		SweepSource::Local => Line::raw(""),
		SweepSource::DaemonStreamed => Line::raw("Source: alertd daemon (just now, on demand)"),
		SweepSource::DaemonCached { computed_at } => {
			let age = super::render::humanise_age_since(*computed_at);
			Line::raw(format!(
				"Source: alertd daemon (computed {age} ago, at {computed_at})"
			))
		}
	}
}

fn build_rows(rows: &[TuiRow], only_failing: bool, spinner: usize) -> Vec<Line<'static>> {
	let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);

	let mut ordered: Vec<&TuiRow> = rows
		.iter()
		.filter(|row| match &row.state {
			RowState::Running => true,
			RowState::Completed(check) => order::keep_under_filter(&check.status, only_failing),
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

fn row_line_running(name: &str, name_width: usize, spinner: usize) -> Line<'static> {
	let frame = SPINNER_FRAMES[spinner % SPINNER_FRAMES.len()];
	let pad = " ".repeat(name_width.saturating_sub(name.len()));
	Line::from(vec![
		Span::raw("  "),
		Span::styled(format!("{frame:<4}"), dim_style()),
		Span::raw("    "),
		Span::raw(name.to_string()),
		Span::raw(pad),
		Span::raw("   "),
		Span::styled("…".to_string(), dim_style()),
	])
}

fn row_line_completed(check: &Check, name_width: usize) -> Line<'static> {
	let (tag, style) = tag_for(check);
	let pad = " ".repeat(name_width.saturating_sub(check.name.len()));
	Line::from(vec![
		Span::raw("  "),
		Span::styled(format!("{tag:<4}"), style),
		Span::raw("    "),
		Span::raw(check.name.to_string()),
		Span::raw(pad),
		Span::raw("   "),
		Span::raw(check.summary.clone()),
	])
}

fn reason_line(reason: &str, name_width: usize) -> Line<'static> {
	let lead = " ".repeat(10 + name_width + 5);
	Line::from(vec![
		Span::raw(lead),
		Span::styled(reason.to_string(), dim_style()),
	])
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

fn tag_for(check: &Check) -> (&'static str, Style) {
	match &check.status {
		CheckStatus::Pass => ("PASS", Style::new().fg(Color::Green).add_modifier(Modifier::BOLD)),
		CheckStatus::Skip(_) => (
			"SKIP",
			Style::new()
				.fg(Color::DarkGray)
				.add_modifier(Modifier::BOLD),
		),
		CheckStatus::Warning(_) => (
			"WARN",
			Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
		),
		CheckStatus::Broken(_) => (
			"BRKN",
			Style::new()
				.fg(Color::Magenta)
				.add_modifier(Modifier::BOLD),
		),
		CheckStatus::Fail(_) => ("FAIL", Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)),
	}
}

fn footer_line(rows: &[TuiRow], total: usize, spinner: usize) -> Line<'static> {
	let completed = rows
		.iter()
		.filter(|r| matches!(r.state, RowState::Completed(_)))
		.count();
	let frame = SPINNER_FRAMES[spinner % SPINNER_FRAMES.len()];
	Line::raw(format!("{frame} {completed} / {total} complete"))
}

fn dim_style() -> Style {
	Style::new().add_modifier(Modifier::DIM)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn line_text(line: &Line<'_>) -> String {
		line.spans.iter().map(|s| s.content.as_ref()).collect()
	}

	#[test]
	fn build_rows_orders_running_then_pass_skip_warn_broken_fail() {
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
				name: "b-skip",
				state: RowState::Completed(Check::skip("b-skip", "n/a", "r")),
			},
			TuiRow {
				name: "c-broken",
				state: RowState::Completed(Check::broken("c-broken", "broke", "r")),
			},
		];
		let lines = build_rows(&rows, false, 0);
		let joined: Vec<String> = lines.iter().map(line_text).collect();
		let positions = |needle: &str| {
			joined
				.iter()
				.position(|s| s.contains(needle))
				.unwrap_or_else(|| panic!("missing {needle}"))
		};
		assert!(positions("k-run") < positions("a-pass"));
		assert!(positions("a-pass") < positions("b-skip"));
		assert!(positions("b-skip") < positions("m-warn"));
		assert!(positions("m-warn") < positions("c-broken"));
		assert!(positions("c-broken") < positions("z-fail"));
	}

	#[test]
	fn build_rows_only_failing_drops_completed_pass_and_skip_but_keeps_running() {
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
		let lines = build_rows(&rows, true, 0);
		let joined: Vec<String> = lines.iter().map(line_text).collect();
		assert!(joined.iter().any(|s| s.contains("d-run")));
		assert!(joined.iter().any(|s| s.contains("c-warn")));
		assert!(!joined.iter().any(|s| s.contains("a-pass")));
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
		let line = footer_line(&rows, 3, 0);
		assert!(line_text(&line).contains("2 / 3 complete"));
	}
}
