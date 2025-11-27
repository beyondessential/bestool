use std::collections::HashSet;

use crate::{column_extractor::ColumnRef, theme::Theme};

#[derive(Clone, Debug)]
pub struct Config {
	/// Syntax highlighting theme
	pub theme: Theme,

	/// Path to audit database directory
	pub audit_path: Option<std::path::PathBuf>,

	/// Whether write mode is enabled upon entering the REPL
	pub write: bool,

	/// Whether to use colours in output
	pub use_colours: bool,

	/// Whether redaction mode is enabled
	pub redact_mode: bool,

	/// Set of columns to redact
	pub redactions: HashSet<ColumnRef>,
}

impl Default for Config {
	fn default() -> Self {
		Self {
			theme: Theme::Dark,
			audit_path: None,
			write: false,
			use_colours: true,
			redact_mode: false,
			redactions: HashSet::new(),
		}
	}
}
