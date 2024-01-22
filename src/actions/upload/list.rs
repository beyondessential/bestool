use aws_sdk_s3::Client as S3Client;
use clap::Parser;
use miette::Result;
use tracing::{info, instrument};

use crate::{actions::Context, aws};

use super::UploadArgs;

/// Display a list of all ongoing uploads for a bucket.
///
/// Shows uploads that are currently ongoing, and their status summary.
///
/// This MUST be run from your local, trusted computer, using WriteAccess or AdminAccess to the
/// account that contains the bucket.
#[derive(Debug, Clone, Parser)]
pub struct ListArgs {
	/// AWS S3 bucket to query.
	#[arg(long, value_name = "BUCKET")]
	pub bucket: String,

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

crate::aws::standard_aws_args!(ListArgs);

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<UploadArgs, ListArgs>) -> Result<()> {
	let aws = aws::init(&ctx.args_sub).await;
	let _client = S3Client::new(&aws);

	info!("Listing ongoing multipart upload");
	// client
	// 	.abort_multipart_upload()
	// 	.bucket(id.bucket)
	// 	.key(id.key)
	// 	.upload_id(id.id)
	// 	.send()
	// 	.await
	// 	.into_diagnostic()?;

	Ok(())
}
