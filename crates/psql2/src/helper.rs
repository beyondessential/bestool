use std::borrow::Cow;

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::{CmdKind, Highlighter};
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Context, Helper};
use syntect::{
	easy::HighlightLines, highlighting::ThemeSet, parsing::SyntaxSet,
	util::as_24_bit_terminal_escaped,
};

use crate::highlighter::Theme;

pub struct SqlHelper {
	syntax_set: SyntaxSet,
	theme_set: ThemeSet,
	theme: Theme,
}

impl SqlHelper {
	pub fn new(theme: Theme) -> Self {
		Self {
			syntax_set: SyntaxSet::load_defaults_newlines(),
			theme_set: ThemeSet::load_defaults(),
			theme,
		}
	}
}

impl Completer for SqlHelper {
	type Candidate = Pair;

	fn complete(
		&self,
		_line: &str,
		_pos: usize,
		_ctx: &Context<'_>,
	) -> rustyline::Result<(usize, Vec<Pair>)> {
		Ok((0, vec![]))
	}
}

impl Hinter for SqlHelper {
	type Hint = String;

	fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
		None
	}
}

impl Highlighter for SqlHelper {
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

impl Validator for SqlHelper {
	fn validate(&self, _ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
		Ok(ValidationResult::Valid(None))
	}
}

impl Helper for SqlHelper {}
