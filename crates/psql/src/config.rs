use bestool_postgres::pool::PgPool;

use crate::theme::Theme;

#[derive(Clone, Debug)]
pub struct Config {
	/// Database connection pool
	pub pool: PgPool,

	/// Syntax highlighting theme
	pub theme: Theme,

	/// Path to audit database directory
	pub audit_path: Option<std::path::PathBuf>,

	/// Whether write mode is enabled upon entering the REPL
	pub write: bool,

	/// Whether to use colours in output
	pub use_colours: bool,
}
