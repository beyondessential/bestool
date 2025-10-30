use std::borrow::Cow;

use rustyline::{
	completion::{Completer, Pair},
	highlight::{CmdKind, Highlighter},
	hint::Hinter,
	validate::{ValidationContext, ValidationResult, Validator},
	Context, Helper,
};
use syntect::{easy::HighlightLines, util::as_24_bit_terminal_escaped};

use crate::highlighter::Theme;

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
				escaped.push_str("\x1b[0m");
				Cow::Owned(escaped)
			}
			Err(_) => Cow::Borrowed(line),
		}
	}

	fn highlight_char(&self, _line: &str, _pos: usize, _forced: CmdKind) -> bool {
		true
	}
}

impl Validator for super::SqlCompleter {
	fn validate(&self, _ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
		Ok(ValidationResult::Valid(None))
	}
}

impl Helper for super::SqlCompleter {}
