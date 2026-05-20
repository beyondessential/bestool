use std::{
	borrow::Cow,
	time::{Duration, Instant},
};

use rustyline::{
	Context, Helper,
	completion::{Completer, Pair},
	highlight::{CmdKind, Highlighter},
	hint::Hinter,
	validate::{ValidationContext, ValidationResult, Validator},
};
use syntect::{easy::HighlightLines, util::as_24_bit_terminal_escaped};

use crate::{colors, repl::TransactionState, theme::Theme};

/// Minimum gap between two consecutive non-forced highlight refreshes.
///
/// Syntect re-highlights the entire line, which is O(line length); without
/// debouncing, a paste streamed character-by-character (e.g. on terminals
/// without bracketed paste) drives O(n²) work and visibly slows the typing.
/// Human keystrokes are much further apart than this, so single-character
/// edits still always redraw.
const HIGHLIGHT_DEBOUNCE: Duration = Duration::from_millis(50);

/// Apply ANSI color codes to the prompt based on write mode and transaction state
fn style_prompt(prompt: &str, write_mode: bool, transaction_state: TransactionState) -> String {
	if write_mode {
		let color_code = match transaction_state {
			TransactionState::Error => colors::prompt_error_code(),
			TransactionState::Active => colors::prompt_transaction_code(),
			TransactionState::Idle | TransactionState::None => colors::prompt_write_mode_code(),
		};

		format!("{color_code}{prompt}{}", colors::reset_code())
	} else {
		prompt.to_string()
	}
}

impl Completer for super::SqlCompleter {
	type Candidate = Pair;

	fn complete(
		&self,
		line: &str,
		pos: usize,
		_ctx: &Context<'_>,
	) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
		let completions = self.find_completions(line, pos);

		let text_before_cursor = &line[..pos];
		let word_start = text_before_cursor
			.rfind(|c: char| c.is_whitespace() || c == '(' || c == ',' || c == ';')
			.map(|i| i + 1)
			.unwrap_or(0);

		Ok((word_start, completions))
	}
}

impl Hinter for super::SqlCompleter {
	type Hint = String;

	fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<Self::Hint> {
		None
	}
}

impl Highlighter for super::SqlCompleter {
	fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
		let syntax = self
			.syntax_set
			.find_syntax_by_extension("sql")
			.unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

		let resolved_theme = self.theme.resolve();
		let theme_name = match resolved_theme {
			Theme::Light => "base16-ocean.light",
			Theme::Dark => "base16-ocean.dark",
			Theme::Auto => unreachable!("resolve() always returns Light or Dark"),
		};

		let theme = &self.theme_set.themes[theme_name];

		let mut highlighter = HighlightLines::new(syntax, theme);
		match highlighter.highlight_line(line, &self.syntax_set) {
			Ok(ranges) => {
				let mut escaped = as_24_bit_terminal_escaped(&ranges[..], false);
				escaped.push_str(&colors::reset_code());
				Cow::Owned(escaped)
			}
			Err(_) => Cow::Borrowed(line),
		}
	}

	fn highlight_char(&self, _line: &str, _pos: usize, kind: CmdKind) -> bool {
		if matches!(kind, CmdKind::ForcedRefresh) {
			*self.last_highlight.lock().unwrap() = Some(Instant::now());
			return true;
		}

		let mut last = self.last_highlight.lock().unwrap();
		let now = Instant::now();
		let should = last.is_none_or(|t| now.duration_since(t) >= HIGHLIGHT_DEBOUNCE);
		*last = Some(now);
		should
	}

	fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
		&'s self,
		prompt: &'p str,
		_default: bool,
	) -> Cow<'b, str> {
		let (write_mode, transaction_state) = if let Some(repl_state) = &self.repl_state {
			let state = repl_state.lock().unwrap();
			(state.write_mode, state.transaction_state)
		} else {
			(false, TransactionState::None)
		};

		Cow::Owned(style_prompt(prompt, write_mode, transaction_state))
	}
}

impl Validator for super::SqlCompleter {
	fn validate(&self, _ctx: &mut ValidationContext<'_>) -> rustyline::Result<ValidationResult> {
		Ok(ValidationResult::Valid(None))
	}
}

impl Helper for super::SqlCompleter {}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::completer::SqlCompleter;

	#[test]
	fn debounces_bursty_highlight_char() {
		let c = SqlCompleter::new(Theme::Dark);

		// First call after construction always refreshes.
		assert!(c.highlight_char("a", 1, CmdKind::Other));
		// A second call immediately after is suppressed.
		assert!(!c.highlight_char("ab", 2, CmdKind::Other));
		assert!(!c.highlight_char("abc", 3, CmdKind::Other));

		// After the debounce window elapses, the next call refreshes again.
		std::thread::sleep(HIGHLIGHT_DEBOUNCE * 2);
		assert!(c.highlight_char("abcd", 4, CmdKind::Other));
	}

	#[test]
	fn forced_refresh_bypasses_debounce() {
		let c = SqlCompleter::new(Theme::Dark);

		assert!(c.highlight_char("a", 1, CmdKind::Other));
		// Even mid-burst, a forced refresh must always redraw.
		assert!(c.highlight_char("ab", 2, CmdKind::ForcedRefresh));
	}
}
