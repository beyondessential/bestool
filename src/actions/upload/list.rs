use aws_sdk_s3::Client as S3Client;
use clap::Parser;
use miette::Result;
use tracing::{info, instrument};

use crate::{
	actions::Context,
	aws::{self, AwsArgs},
};

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

	#[command(flatten)]
	pub aws: AwsArgs,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<UploadArgs, ListArgs>) -> Result<()> {
	let aws = aws::init(&ctx.args_sub.aws).await;
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
