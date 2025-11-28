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

/// Convert crossterm Color to comfy_table Color
pub fn to_comfy_color(color: Color) -> comfy_table::Color {
	match color {
		Color::Black => comfy_table::Color::Black,
		Color::DarkGrey => comfy_table::Color::DarkGrey,
		Color::Red => comfy_table::Color::Red,
		Color::DarkRed => comfy_table::Color::DarkRed,
		Color::Green => comfy_table::Color::Green,
		Color::DarkGreen => comfy_table::Color::DarkGreen,
		Color::Yellow => comfy_table::Color::Yellow,
		Color::DarkYellow => comfy_table::Color::DarkYellow,
		Color::Blue => comfy_table::Color::Blue,
		Color::DarkBlue => comfy_table::Color::DarkBlue,
		Color::Magenta => comfy_table::Color::Magenta,
		Color::DarkMagenta => comfy_table::Color::DarkMagenta,
		Color::Cyan => comfy_table::Color::Cyan,
		Color::DarkCyan => comfy_table::Color::DarkCyan,
		Color::White => comfy_table::Color::White,
		Color::Grey => comfy_table::Color::Grey,
		Color::Rgb { r, g, b } => comfy_table::Color::Rgb { r, g, b },
		Color::AnsiValue(v) => comfy_table::Color::AnsiValue(v),
		_ => comfy_table::Color::Reset,
	}
}

/// Clear current line
pub const CLEAR_LINE: &str = "\r\x1b[K";
