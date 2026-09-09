//! The command-line surface over the read API.
//!
//! `bestool-psql-audit` and `bestool audit-psql` are the same tool reached two
//! ways, so both take these arguments and run them through here.
//!
//! spec: AUD-API

use std::{io::Write as _, path::PathBuf};

use clap::{Parser, Subcommand};
use miette::Result;

use super::tools::{self, QueryOptions};

/// Read and maintain the bestool-psql audit log.
#[derive(Debug, Clone, Parser)]
pub struct AuditArgs {
	/// Path to the audit directory
	#[arg(long, value_name = "PATH", help = help_audit_path())]
	pub audit_path: Option<PathBuf>,

	#[command(subcommand)]
	pub command: Option<AuditCommand>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum AuditCommand {
	/// Write records to standard output, in the shape and framing they have in
	/// the store
	Export(ExportArgs),

	/// Check every session's hash chain, and exit non-zero if any does not hold
	Verify,

	/// Fold closed segments into day files and delete days past retention
	Compact,
}

#[derive(Debug, Clone, Default, Parser)]
pub struct ExportArgs {
	/// Number of records to return (0 = unlimited)
	#[arg(short = 'n', long, default_value = "100")]
	pub limit: Option<usize>,

	/// Take the oldest records rather than the newest
	#[arg(long)]
	pub first: bool,

	/// Only records at or after this time
	#[arg(long)]
	pub since: Option<String>,

	/// Only records at or before this time
	#[arg(long)]
	pub until: Option<String>,
}

impl From<ExportArgs> for QueryOptions {
	fn from(args: ExportArgs) -> Self {
		Self {
			limit: args.limit,
			from_oldest: args.first,
			since: args.since,
			until: args.until,
		}
	}
}

fn help_audit_path() -> String {
	format!(
		"Path to the audit directory (default: {})",
		super::default_path_for_help()
	)
}

/// Run the tool.
///
/// Returns false when a chain does not hold, which the caller turns into a
/// non-zero exit.
pub fn run_audit_cli(args: AuditArgs) -> Result<bool> {
	let dir = match args.audit_path {
		Some(path) => path,
		None => super::default_path()?,
	};

	// A tool reading a machine that has not run a session since still sees what
	// a session would.
	tools::import_if_legacy(&dir)?;

	match args.command.unwrap_or(AuditCommand::Export(ExportArgs {
		limit: Some(100),
		..Default::default()
	})) {
		AuditCommand::Export(args) => {
			let mut out = std::io::stdout().lock();
			match tools::write_export(&mut out, &dir, &args.into()) {
				// A closed output pipe ends the export quietly.
				Err(err) if tools::is_broken_pipe(&err) => Ok(true),
				Err(err) => Err(err),
				Ok(()) => Ok(true),
			}
		}

		AuditCommand::Verify => {
			let report = tools::verify_directory(&dir)?;
			let mut out = std::io::stderr().lock();

			for session in &report.sessions {
				let state = if session.holds() { "holds" } else { "BROKEN" };
				writeln!(
					out,
					"{} {state}: {} records, head {}",
					session.instance, session.records, session.head
				)
				.ok();

				if session.truncated_start {
					writeln!(
						out,
						"  oldest kept record is unverifiable: earlier records have been deleted by retention"
					)
					.ok();
				}
				for (seq, gap) in &session.gaps {
					writeln!(
						out,
						"  gap at {seq}: {} records lost through {}, {} to {}",
						gap.lost, gap.through, gap.from, gap.to
					)
					.ok();
				}
				if let Some(broken) = &session.broken_at {
					writeln!(
						out,
						"  breaks at record {} ({}): {}",
						broken.seq, broken.ts, broken.reason
					)
					.ok();
				}
			}

			for skipped in &report.skipped {
				writeln!(
					out,
					"{} bytes at offset {} in {} are not a record",
					skipped.bytes,
					skipped.at,
					skipped.file.display()
				)
				.ok();
			}
			for renamed in &report.renamed {
				writeln!(
					out,
					"{} is named for session {} but its records were written by {}",
					renamed.file.display(),
					renamed.names,
					renamed.records
				)
				.ok();
			}
			if report.unattributed > 0 {
				writeln!(
					out,
					"{} records belong to no session in the log",
					report.unattributed
				)
				.ok();
			}

			if report.sessions.is_empty() {
				writeln!(out, "the audit log is empty").ok();
			}

			Ok(report.holds())
		}

		AuditCommand::Compact => {
			let report = tools::compact_directory(&dir)?;
			let mut out = std::io::stderr().lock();

			if report.skipped {
				writeln!(out, "another process is compacting this directory").ok();
				return Ok(true);
			}
			for (date, segments) in &report.folded {
				writeln!(out, "folded {segments} segments covering {date}").ok();
			}
			for date in &report.expired {
				writeln!(out, "deleted {date}, past the retention period").ok();
			}
			if report.did_nothing() {
				writeln!(out, "nothing to compact").ok();
			}

			Ok(true)
		}
	}
}
