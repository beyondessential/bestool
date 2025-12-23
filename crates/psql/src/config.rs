use std::{collections::HashSet, sync::Arc};

use crate::{column_extractor::ColumnRef, theme::Theme};

pub trait SnippetLookupProvider: Send + Sync {
	fn lookup(&self, name: &str) -> Option<String>;
	fn list_names(&self) -> Vec<String> {
		Vec::new()
	}
	fn get_description(&self, name: &str) -> Option<String> {
		let _ = name;
		None
	}
}

pub type SnippetLookup = Arc<dyn SnippetLookupProvider>;

#[derive(Clone)]
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

	/// Optional provider for custom snippet lookup
	pub snippet_lookup: Option<SnippetLookup>,
}

impl std::fmt::Debug for Config {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Config")
			.field("theme", &self.theme)
			.field("audit_path", &self.audit_path)
			.field("write", &self.write)
			.field("use_colours", &self.use_colours)
			.field("redact_mode", &self.redact_mode)
			.field("redactions", &self.redactions)
			.field(
				"snippet_lookup",
				&self.snippet_lookup.as_ref().map(|_| "<closure>"),
			)
			.finish()
	}
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
			snippet_lookup: None,
		}
	}
}
