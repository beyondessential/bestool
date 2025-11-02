use std::path::PathBuf;

use bestool_psql::{ExportOptions, QueryOptions, default_audit_dir, export_audit_entries};
use clap::Parser;
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{Result, miette};
use tracing::debug;

/// Export audit database entries as JSON
#[derive(Debug, Clone, Parser)]
pub struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	/// Path to audit database directory
	#[arg(long, value_name = "PATH", help = help_audit_path())]
	pub audit_path: Option<PathBuf>,

	/// Number of entries to return (0 = unlimited)
	#[arg(short = 'n', long, default_value = "100")]
	pub limit: Option<usize>,

	/// Read from oldest entries instead of newest
	#[arg(long)]
	pub first: bool,

	/// Filter entries after this date
	#[arg(long)]
	pub since: Option<String>,

	/// Filter entries before this date
	#[arg(long)]
	pub until: Option<String>,

	/// Discover and read orphan databases instead of main database
	#[arg(long)]
	pub orphans: bool,
}

fn help_audit_path() -> String {
	format!(
		"Path to audit database directory (default: {})",
		default_audit_dir()
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

	let options = ExportOptions {
		audit_path: args.audit_path,
		query_options: QueryOptions {
			limit: args.limit,
			from_oldest: args.first,
			since: args.since,
			until: args.until,
		},
		orphans: args.orphans,
	};

	// Handle broken pipe gracefully
	match export_audit_entries(options) {
		Ok(()) => Ok(()),
		Err(e) => {
			// Check if this is a broken pipe error by examining the error message
			let error_msg = format!("{:?}", e);
			if error_msg.contains("Broken pipe") || error_msg.contains("BrokenPipe") {
				return Ok(());
			}
			Err(e)
		}
	}
}
