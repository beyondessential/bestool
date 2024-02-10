use std::{fs::File, path::PathBuf};

use clap::Parser;

use leon::Template;
use miette::{bail, Context as _, IntoDiagnostic, Result};
use minisign::sign;
use tracing::debug;

use super::{inout_args::inout_files, key_args::SecretKeyArgs, Context, SignArgs};

/// Sign a file or data with a secret key.
#[derive(Debug, Clone, Parser)]
pub struct FilesArgs {
	/// A file to sign.
	///
	/// You can provide this multiple times to sign multiple files.
	pub files: Vec<PathBuf>,

	#[command(flatten)]
	pub key: SecretKeyArgs,

	/// The output file to write the signature to.
	///
	/// If not provided at all, the signature will be written to the same file as the input, with a
	/// `.sig` extension appended.
	///
	/// If provided, and multiple files are being signed, this must be provided as many times as
	/// there are input files, or once but include one of the two following placeholders, in which
	/// case it is treated as a template: `{filename}` will be replaced with the input filename, and
	/// `{n}` will be replaced with an incrementing number (from 1).
	#[arg(long, value_name = "FILE")]
	pub output: Vec<PathBuf>,

	/// The "trusted comment" to include in the signature.
	///
	/// This is a free-form string that is included in the signature, and can be used to store
	/// metadata about the signature or file; it is part of the data being signed, so can be trusted
	/// in the same way as the signed file itself upon verification.
	///
	/// If this contains any of the following placeholders, they will be replaced: `{filename}` with
	/// the input filename, `{n}` with an incrementing number (from 1), `{keyid}` with the key ID of
	/// the signing key, and `{timestamp}` with the current date and time in RFC3339 format.
	#[arg(long, value_name = "COMMENT")]
	pub comment: Option<String>,
}

pub async fn run(ctx: Context<SignArgs, FilesArgs>) -> Result<()> {
	let FilesArgs {
		files,
		key,
		output,
		comment,
	} = ctx.args_sub;
	let sk = key.read()?;

	let output_names = inout_files(output, &files)?;

	let comment = if let Some(comment) = comment.as_ref() {
		Some(Template::parse(comment)?)
	} else {
		None
	};

	let now = chrono::Utc::now().to_rfc3339();
	let keyid = hex::encode(sk.keynum());

	let mut errors = 0;
	for (n, (file, output)) in files.iter().zip(output_names).enumerate() {
		if let Err(error) = sign_file(file, output, &comment, n, &keyid, &now, &sk).await {
			errors += 1;
			eprintln!("failed signing file, skipping\n{error}");
		}
	}

	if errors > 0 {
		bail!("failed to sign {errors} files");
	}

	Ok(())
}

async fn sign_file(
	file: &PathBuf,
	output: PathBuf,
	comment: &Option<Template<'_>>,
	n: usize,
	keyid: &String,
	now: &String,
	sk: &minisign::SecretKey,
) -> Result<(), miette::ErrReport> {
	debug!(?file, ?output, "signing");
	let reader = File::open(file)
		.into_diagnostic()
		.wrap_err(format!("failed reading {file:?}"))?;

	let comment = comment
		.as_ref()
		.map(|t| {
			t.render(&[
				("filename", file.to_string_lossy().as_ref()),
				("n", (n + 1).to_string().as_ref()),
				("keyid", keyid),
				("timestamp", now),
			])
		})
		.transpose()
		.wrap_err(format!("failed rendering comment for {file:?}"))?;

	let signature = sign(None, sk, reader, comment.as_deref(), None)
		.into_diagnostic()
		.wrap_err(format!("failed signing {file:?}"))?;

	tokio::fs::write(&output, signature.into_string())
		.await
		.into_diagnostic()
		.wrap_err(format!("failed writing signature to {output:?}"))?;

	eprintln!("signed {file:?} -> {output:?}");
	Ok(())
}
