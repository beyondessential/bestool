use std::{fs::File, path::PathBuf};

use clap::Parser;

use miette::{bail, Context as _, IntoDiagnostic, Result};
use minisign::{verify, SignatureBox};
use tracing::{debug, error};

use super::{inout_args::inout_files, key_args::PublicKeyArgs, Context, SignArgs};

/// Check a file against a public key and signature.
#[derive(Debug, Clone, Parser)]
pub struct CheckArgs {
	/// A file to check.
	///
	/// You can provide this multiple times to check multiple files; in this case, the signatures
	/// must be provided via `--sig-file`.
	pub files: Vec<PathBuf>,

	#[command(flatten)]
	pub key: PublicKeyArgs,

	/// The signature file to read the signature from.
	///
	/// If not provided at all, the signature will be read from the same file as the input, with a
	/// `.sig` extension appended.
	///
	/// If provided, and multiple files are being checked, this must be provided as many times as
	/// there are input files, or once but include one of the two following placeholders, in which
	/// case it is treated as a template: `{filename}` will be replaced with the input filename, and
	/// `{n}` will be replaced with an incrementing number (from 1).
	#[arg(long, value_name = "FILE")]
	pub sig_file: Vec<PathBuf>,

	/// Don't print anything to the console, only return the exit code.
	///
	/// This is useful for scripting, where you only care about whether the signature is valid or
	/// not. By default the trusted comment is printed, if present, and the message "OK" or "BAD".
	#[arg(long, short)]
	pub quiet: bool,
}

pub async fn run(ctx: Context<SignArgs, CheckArgs>) -> Result<()> {
	let CheckArgs {
		files,
		key,
		sig_file,
		quiet,
	} = ctx.args_sub;
	let pk = key.read()?;

	let sigfiles = inout_files(sig_file, &files)?;

	let mut errors = 0;
	for (infile, sigfile) in files.iter().zip(sigfiles) {
		if !sigfile.exists() {
			error!(?infile, ?sigfile, "signature file does not exist");
			errors += 1;
			if !quiet { eprintln!("checked {infile:?}: MISSING SIG") };
			continue;
		}

		if let Err(error) = check_file(infile, &sigfile, &pk, quiet) {
			error!(?infile, ?sigfile, "checking error: {error}");
			errors += 1;
			if !quiet { eprintln!("checked {infile:?}: BAD") };
		}
	}

	if errors > 0 {
		bail!("failed to check {errors} files");
	}

	Ok(())
}

fn check_file(
	infile: &PathBuf,
	sigfile: &PathBuf,
	pk: &minisign::PublicKey,
	quiet: bool,
) -> Result<(), miette::ErrReport> {
	debug!(?infile, ?sigfile, "checking");
	let reader = File::open(infile)
		.into_diagnostic()
		.wrap_err(format!("failed opening {infile:?}"))?;

	let signature = SignatureBox::from_file(sigfile)
		.into_diagnostic()
		.wrap_err(format!("failed reading signature from {sigfile:?}"))?;

	verify(pk, &signature, reader, true, false, false)
		.into_diagnostic()?;

	if !quiet {
		let comment = signature.trusted_comment().into_diagnostic()?;
		if comment.is_empty() {
			eprintln!("checked {infile:?}: OK");
		} else {
			eprintln!("checked {infile:?}: OK ({comment})");
		}
	}

	Ok(())
}
