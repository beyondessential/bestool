use std::path::PathBuf;

use aws_sdk_s3::Client as S3Client;
use clap::Parser;
use miette::{IntoDiagnostic, Result};
use tokio::{fs::File, io::AsyncReadExt};
use tracing::{info, instrument};

use crate::{
	actions::{upload::decode_token, Context},
	aws,
};

use super::UploadArgs;

/// Cancel a pre-auth'ed upload.
///
/// Given a pre-auth token or Upload ID (as generated by `bestool upload preauth`), cancel the
/// upload it references. If the upload is ongoing, this will cause it to immediately fail.
///
/// This MUST be run from your local, trusted computer, using WriteAccess or AdminAccess to the
/// account that contains the destination bucket.
#[derive(Debug, Clone, Parser)]
pub struct CancelArgs {
	/// File which contains the token to cancel.
	#[arg(long, value_name = "FILENAME", default_value = "token.txt", required_unless_present_any = &["token", "upload-id"])]
	pub token_file: PathBuf,

	/// Token value.
	///
	/// This is the token to cancel. If not specified here, it will be taken from the file specified
	/// in `--token-file`. Prefer to use `--token-file` instead of this option, as tokens are
	/// generally larger than can be passed on the command line.
	#[arg(long, value_name = "TOKEN")]
	pub token: Option<String>,

	/// Upload ID.
	///
	/// This is the Upload ID to cancel. If not specified here, it will be taken from `--token` or
	/// `--token-file`.
	#[arg(long, value_name = "UPLOAD_ID", conflicts_with_all = &["token", "token_file"])]
	pub upload_id: Option<String>,

	/// AWS Access Key ID.
	///
	/// This is the AWS Access Key ID to use for authentication. If not specified here, it will be
	/// taken from the environment variable `AWS_ACCESS_KEY_ID`, or from the AWS credentials file
	/// (usually `~/.aws/credentials`), or from ambient credentials (eg EC2 instance profile).
	#[arg(long, value_name = "KEY_ID")]
	pub aws_access_key_id: Option<String>,

	/// AWS Secret Access Key.
	///
	/// This is the AWS Secret Access Key to use for authentication. If not specified here, it will
	/// be taken from the environment variable `AWS_SECRET_ACCESS_KEY`, or from the AWS credentials
	/// file (usually `~/.aws/credentials`), or from ambient credentials (eg EC2 instance profile).
	#[arg(long, value_name = "SECRET_KEY")]
	pub aws_secret_access_key: Option<String>,

	/// AWS Region.
	///
	/// This is the AWS Region to use for authentication and for the bucket. If not specified here,
	/// it will be taken from the environment variable `AWS_REGION`, or from the AWS credentials
	/// file (usually `~/.aws/credentials`), or from ambient credentials (eg EC2 instance profile).
	#[arg(long, value_name = "REGION")]
	pub aws_region: Option<String>,
}

crate::aws::standard_aws_args!(CancelArgs);

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<UploadArgs, CancelArgs>) -> Result<()> {
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

	let token = decode_token(&token)?;

	let aws = aws::init(&ctx.args_sub).await;
	let client = S3Client::new(&aws);

	info!(?token.id, "Cancelling multipart upload");
	client
		.abort_multipart_upload()
		.bucket(token.id.bucket)
		.key(token.id.key)
		.upload_id(token.id.id)
		.send()
		.await
		.into_diagnostic()?;
	info!("Upload cancelled");

	Ok(())
}
