//! Color utilities for consistent styling across the application using crossterm.
//!
//! This module provides reusable color codes and styling functions to avoid
//! hardcoded ANSI codes and ensure consistent appearance across platforms.

use crossterm::style::{Color, ResetColor, SetForegroundColor, Stylize};

/// The redacted value placeholder
pub const REDACTED_VALUE: &str = "[redacted]";

/// Colors used throughout the application
pub struct Colors;

impl Colors {
	/// Status messages (row counts, timing info)
	pub const STATUS: Color = Color::Magenta;

	/// Error states in prompts
	pub const ERROR: Color = Color::Red;

	/// Active transaction in prompts
	pub const TRANSACTION: Color = Color::Cyan;

	/// Normal write mode in prompts
	pub const WRITE_MODE: Color = Color::Green;

	/// Truncation/warning messages
	pub const WARNING: Color = Color::Yellow;

	/// Progress indicators
	pub const PROGRESS: Color = Color::Magenta;

	/// Redacted values
	pub const REDACTED: Color = Color::Yellow;
}

/// Style a status message (e.g., "(N rows, took X ms)")
pub fn style_status(text: &str, use_colours: bool) -> String {
	if use_colours {
		format!("{}", text.with(Colors::STATUS).dim())
	} else {
		text.to_string()
	}
}

/// Style a warning/truncation message
pub fn style_warning(text: &str, use_colours: bool) -> String {
	if use_colours {
		format!("{}", text.with(Colors::WARNING).bold())
	} else {
		text.to_string()
	}
}

/// Style a progress message
pub fn style_progress(text: &str, use_colours: bool) -> String {
	if use_colours {
		format!("{}", text.with(Colors::PROGRESS).dim())
	} else {
		text.to_string()
	}
}

/// Get ANSI code for error prompt color (bold red)
pub fn prompt_error_code() -> String {
	format!("{}", SetForegroundColor(Colors::ERROR))
}

/// Get ANSI code for transaction prompt color (bold blue)
pub fn prompt_transaction_code() -> String {
	format!("{}", SetForegroundColor(Colors::TRANSACTION))
}

/// Get ANSI code for write mode prompt color (bold green)
pub fn prompt_write_mode_code() -> String {
	format!("{}", SetForegroundColor(Colors::WRITE_MODE))
}

/// Get ANSI code to reset colors
pub fn reset_code() -> String {
	format!("{}", ResetColor)
}

/// Style a redacted value
pub fn style_redacted(use_colours: bool) -> String {
	if use_colours {
		format!("{}", REDACTED_VALUE.with(Colors::REDACTED))
	} else {
		REDACTED_VALUE.to_string()
	}
}

/// Clear current line
pub const CLEAR_LINE: &str = "\r\x1b[K";
