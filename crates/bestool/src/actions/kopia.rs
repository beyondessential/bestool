use clap::{Parser, Subcommand};
use miette::Result;

use crate::args::Args;

use super::Context;

pub mod common;

/// Operate on a kopia repository.
///
/// Wraps the `kopia` CLI to add ergonomics for our deployments: defaults
/// scoped to the current host, snapshot pickers, and on Linux a transparent
/// re-exec under the system `kopia` user so the operator doesn't need to
/// remember `sudo -u kopia`.
#[derive(Debug, Clone, Parser)]
pub struct KopiaArgs {
	/// Don't auto re-exec under the `kopia` user on Linux.
	///
	/// By default, when running as a non-`kopia` user on Linux and the
	/// system kopia install is present, the command re-execs itself via
	/// `sudo -u kopia --` so it can read the system kopia config (which is
	/// owned by the `kopia` user). This flag opts out — useful when you've
	/// set up your own kopia config under your own user account.
	#[arg(long, global = true)]
	pub no_sudo: bool,

	/// Override the kopia binary location.
	///
	/// By default the command searches for `kopia` in `PATH`, then falls
	/// back to known KopiaUI install locations on Windows.
	#[arg(long, global = true, value_name = "PATH")]
	pub kopia_bin: Option<std::path::PathBuf>,

	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[KopiaArgs => |args: KopiaArgs, mut ctx: Context| -> Result<(Action, Context)> {
		let _top: &Args = ctx.require();
		common::maybe_reexec_as_kopia(&args)?;
		let action = args.action.clone();
		ctx.provide(args);
		Ok((action, ctx))
	}]

	info => Info(InfoArgs)
}
