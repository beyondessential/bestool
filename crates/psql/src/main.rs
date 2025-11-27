use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{IntoDiagnostic, Result, miette};
use tracing::debug;

/// Async PostgreSQL client
#[derive(Debug, Clone, Parser)]
pub struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	/// Generate markdown documentation
	#[arg(long, hide = true)]
	pub docs: bool,

	/// Database name or connection URL
	///
	/// Can be a simple database name (e.g., 'mydb') or full connection string
	/// (e.g., 'postgresql://user:password@localhost:5432/dbname')
	pub connstring: Option<String>,

	/// TLS mode for the connection (if sslmode is not set in the URL).
	///
	/// Defaults to 'prefer' which attempts TLS but falls back to non-TLS.
	/// Use 'disable' to skip TLS entirely (useful on Windows with certificate issues).
	/// Use 'require' to enforce TLS connections.
	#[arg(long, value_enum, default_value_t = TlsMode::default())]
	pub ssl: TlsMode,

	/// Enable write mode for this session
	///
	/// By default the session is read-only. To enable writes, pass this flag.
	/// This also disables autocommit, so you need to issue a COMMIT; command
	/// whenever you perform a write (insert, update, etc), as an extra safety measure.
	#[arg(short = 'W', long)]
	pub write: bool,

	/// Syntax highlighting theme (light, dark, or auto)
	///
	/// Controls the color scheme for SQL syntax highlighting in the input line.
	/// 'auto' attempts to detect terminal background, defaults to 'dark' if detection fails.
	#[arg(long, default_value = "auto")]
	pub theme: bestool_psql::Theme,

	/// Path to audit database directory
	#[arg(long, value_name = "PATH", help = help_audit_path())]
	pub audit_path: Option<PathBuf>,
}

/// TLS mode for PostgreSQL connections
#[derive(Debug, Default, Clone, Copy, ValueEnum)]
pub enum TlsMode {
	/// Disable TLS encryption
	Disable,
	/// Prefer TLS but allow unencrypted connections
	#[default]
	Prefer,
	/// Require TLS encryption
	Require,
}

impl TlsMode {
	fn as_str(self) -> &'static str {
		match self {
			TlsMode::Disable => "disable",
			TlsMode::Prefer => "prefer",
			TlsMode::Require => "require",
		}
	}
}

fn help_audit_path() -> String {
	format!(
		"Path to audit database directory (default: {})",
		bestool_psql::default_audit_dir()
	)
}

fn get_args() -> Result<(Args, WorkerGuard)> {
	let log_guard = PreArgs::parse().setup().map_err(|err| miette!("{err}"))?;

	debug!("parsing arguments");
	let args = Args::parse();

	let log_guard = match log_guard {
		Some(g) => g,
		None => args
			.logging
			.setup(|v| match v {
				0 => "bestool_psql=info",
				1 => "info,bestool_psql=debug",
				2 => "debug",
				3 => "debug,bestool_psql=trace",
				_ => "trace",
			})
			.map_err(|err| miette!("{err}"))?,
	};

	debug!(?args, "got arguments");
	Ok((args, log_guard))
}

#[tokio::main]
async fn main() -> Result<()> {
	let (args, _guard) = get_args()?;

	if args.docs || args.connstring.as_ref().is_some_and(|c| c == "_docs") {
		let markdown = clap_markdown::help_markdown::<Args>();
		println!("{}", markdown);
		return Ok(());
	}

	let connstring = args
		.connstring
		.ok_or_else(|| miette!("database name or connection URL is required"))?;

	let theme = args.theme.resolve();
	debug!(?theme, "using syntax highlighting theme");

	let url = if connstring.contains("://") {
		let mut url = url::Url::parse(&connstring).into_diagnostic()?;
		if !url.query_pairs().any(|(key, _)| key == "sslmode") {
			url.query_pairs_mut()
				.append_pair("sslmode", args.ssl.as_str());
		}
		url.to_string()
	} else {
		format!(
			"postgresql://localhost/{}?sslmode={}",
			connstring,
			args.ssl.as_str()
		)
	};
	debug!(url, "using connection url");

	debug!("creating connection pool");
	let pool = bestool_psql::create_pool(&url).await?;

	bestool_psql::register_sigint_handler()?;
	bestool_psql::run(
		pool,
		bestool_psql::Config {
			theme,
			audit_path: args.audit_path,
			write: args.write,
			use_colours: args.logging.color.enabled(),
			..Default::default()
		},
	)
	.await
}
