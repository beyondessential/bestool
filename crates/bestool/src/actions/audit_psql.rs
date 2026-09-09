use bestool_psql::{AuditArgs, run_audit_cli};
use clap::Parser;
use miette::Result;

use crate::actions::Context;

/// Read and maintain the bestool-psql audit log.
#[derive(Debug, Clone, Parser)]
pub struct AuditPsqlArgs {
	#[command(flatten)]
	pub audit: AuditArgs,
}

pub async fn run(args: AuditPsqlArgs, _ctx: Context) -> Result<()> {
	if run_audit_cli(args.audit)? {
		Ok(())
	} else {
		// A chain that does not hold is a non-zero exit, not an error to report
		// twice: verify has already said where and how.
		std::process::exit(1);
	}
}
