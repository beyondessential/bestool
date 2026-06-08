//! Shared SQL lexical-state tracking.
//!
//! Both the REPL statement scanner ([`super::multi`]) and the query executor
//! need to find statement boundaries (`;`, and for the REPL also `\g`) without
//! being fooled by characters that appear inside string literals, dollar-quoted
//! strings, or comments. This module owns that lexical bookkeeping so the two
//! call sites share one implementation instead of each tracking quote state
//! their own way.

/// Lexical state of an in-progress scan over SQL text.
#[derive(Debug, Default)]
pub(crate) struct SqlLexState {
	in_single_quote: bool,
	in_double_quote: bool,
	in_dollar_quote: Option<String>,
	in_line_comment: bool,
	block_comment_depth: usize,
	prev: char,
}

impl SqlLexState {
	/// True when the scanner is in ordinary code — not inside a string,
	/// dollar-quoted body, or comment — i.e. where a `;` or `\g` is a real
	/// statement terminator.
	pub(crate) fn in_code(&self) -> bool {
		!self.in_single_quote
			&& !self.in_double_quote
			&& self.in_dollar_quote.is_none()
			&& !self.in_line_comment
			&& self.block_comment_depth == 0
	}

	/// Advance the state across the token starting at `chars[i]`, appending the
	/// consumed character(s) to `out`. Returns the number of characters consumed
	/// (more than one only for dollar-quote delimiters such as `$tag$`).
	pub(crate) fn step(&mut self, chars: &[char], i: usize, out: &mut String) -> usize {
		let ch = chars[i];

		// Dollar-quote delimiters ($tag$ or $$), recognised both to open a quote
		// and (when already inside one with a matching tag) to close it.
		if ch == '$'
			&& !self.in_single_quote
			&& !self.in_double_quote
			&& !self.in_line_comment
			&& self.block_comment_depth == 0
			&& let Some((tag, len)) = match_dollar_tag(chars, i)
		{
			out.extend(&chars[i..i + len]);
			match &self.in_dollar_quote {
				Some(open) if *open == tag => self.in_dollar_quote = None,
				Some(_) => {}
				None => self.in_dollar_quote = Some(tag),
			}
			self.prev = '$';
			return len;
		}

		match ch {
			_ if self.in_line_comment => {
				if ch == '\n' {
					self.in_line_comment = false;
				}
			}
			_ if self.in_dollar_quote.is_some() => {}
			'\'' if !self.in_double_quote && self.block_comment_depth == 0 && self.prev != '\\' => {
				self.in_single_quote = !self.in_single_quote;
			}
			'"' if !self.in_single_quote && self.block_comment_depth == 0 && self.prev != '\\' => {
				self.in_double_quote = !self.in_double_quote;
			}
			_ if self.in_single_quote || self.in_double_quote => {}
			'*' if self.prev == '/' => {
				self.block_comment_depth += 1;
			}
			'/' if self.prev == '*' && self.block_comment_depth > 0 => {
				self.block_comment_depth -= 1;
			}
			_ if self.block_comment_depth > 0 => {}
			'-' if self.prev == '-' => {
				self.in_line_comment = true;
			}
			_ => {}
		}

		out.push(ch);
		self.prev = ch;
		1
	}
}

/// If a dollar-quote delimiter (`$tag$` or `$$`) starts at `chars[start]`,
/// return its tag (without the dollars) and its length in characters.
fn match_dollar_tag(chars: &[char], start: usize) -> Option<(String, usize)> {
	let mut j = start + 1;
	while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
		j += 1;
	}
	if j < chars.len() && chars[j] == '$' {
		let tag: String = chars[start + 1..j].iter().collect();
		Some((tag, j + 1 - start))
	} else {
		None
	}
}
