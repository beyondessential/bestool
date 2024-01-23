use std::path::PathBuf;

use aws_sdk_s3::Client as S3Client;
use clap::Parser;
use miette::{miette, IntoDiagnostic, Result};
use tokio::{fs::File, io::AsyncReadExt};
use tracing::{info, instrument};

use crate::{
	actions::{upload::token::decode_token, Context},
	aws::{self, AwsArgsFragment},
};

use super::UploadArgs;

/// Query the status of an upload.
///
/// Given a pre-auth token or Upload ID (as generated by `bestool upload preauth`), show when it
/// expires, and how many parts have been uploaded so far, along with the amount of data uploaded.
///
/// This MUST be run from your local, trusted computer, using WriteAccess or AdminAccess to the
/// account that contains the destination bucket.
#[derive(Debug, Clone, Parser)]
pub struct StatusArgs {
	/// File which contains the token to query.
	#[arg(
		long,
		value_name = "FILENAME",
		default_value = "token.txt",
		required_unless_present_any = &["token", "upload_id"],
	)]
	pub token_file: PathBuf,

	/// Token value.
	///
	/// This is the token to query. If not specified here, it will be taken from the file specified
	/// in `--token-file`. Prefer to use `--token-file` instead of this option, as tokens are
	/// generally larger than can be passed on the command line.
	#[arg(long, value_name = "TOKEN")]
	pub token: Option<String>,

	/// Upload ID.
	///
	/// This is the Upload ID to query. If not specified here, it will be taken from `--token` or
	/// `--token-file`.
	#[arg(
		long,
		value_name = "UPLOAD_ID",
		conflicts_with_all = &["token", "token_file"],
	)]
	pub upload_id: Option<String>,

	#[command(flatten)]
	pub aws: AwsArgsFragment,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<UploadArgs, StatusArgs>) -> Result<()> {
	let id = if let Some(upload_id) = ctx.args_sub.upload_id.as_deref() {
		upload_id.parse().map_err(|err| miette!("{}", err))?
	} else {
		let token = if let Some(token) = ctx.args_sub.token.clone() {
			token
		} else {
			let mut file = File::open(&ctx.args_sub.token_file)
				.await
				.into_diagnostic()?;
			let mut token = String::new();
			file.read_to_string(&mut token).await.into_diagnostic()?;
			token
		};

		decode_token(&token)?.id
	};

	let aws = aws::init(&ctx.args_sub.aws).await;
	let client = S3Client::new(&aws);

	info!(?id, "Querying multipart upload status");
	let mut parts = client
		.list_parts()
		.bucket(id.bucket)
		.key(id.key)
		.upload_id(id.id)
		.into_paginator()
		.items()
		.send();

	let mut parts_remaining = id.parts;
	while let Some(part) = parts.next().await {
		parts_remaining -= 1;
		let part = part.into_diagnostic()?;
		eprintln!("{part:?}");
	}

	if parts_remaining > 0 {
		eprintln!("{} parts remaining", parts_remaining);
	}

	Ok(())
}
