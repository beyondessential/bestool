use aws_sdk_s3::{types::BucketVersioningStatus, Client as S3Client};
use clap::Parser;
use miette::{bail, IntoDiagnostic, Result};
use serde_json::json;
use tracing::{debug, info, instrument};

use crate::{
	actions::Context,
	aws::{self, s3::parse_bucket_and_key, token::DelegatedToken, AwsArgs},
};

use super::UploadArgs;

/// Generate a delegated token to upload a file.
///
/// This MUST be run from your local, trusted computer, using WriteAccess or AdminAccess to the
/// account that contains the desired destination bucket. It will generate a delegated token that
/// allows anyone to upload a file to the specified bucket and key, without them needing any AWS
/// credentials.
///
/// Tokens have an expiry date. By default, tokens expire after 12 HOURS. You can specify a longer
/// or shorter expiry time with `--expiry`. Be careful with short expiry times! The token must
/// remain valid for the entire duration of the upload, and if the upload takes longer than the
/// token is valid, it will fail and will need to be retried from scratch, using a new token.
#[derive(Debug, Clone, Parser)]
pub struct DelegateArgs {
	/// AWS S3 bucket to upload to.
	///
	/// This may also contain the key, if given in s3://bucket/key format. See the `--key` option
	/// for semantics of the key portion.
	#[arg(long, value_name = "BUCKET", required = true)]
	pub bucket: String,

	/// Pathname in the bucket to upload to.
	///
	/// Files can only be uploaded to the exact name as specified here, unless wildcards are used.
	///
	/// If this contains one or more wildcards (`*`), files can be uploaded to any key that matches
	/// the wildcard pattern (be very careful with this!). When using wildcards, always enclose the
	/// value in quotes, to prevent the shell from expanding the wildcard. You also need to specify
	/// `--allow-wildcards`, to prevent mishaps.
	///
	/// You can also give the key via the `--bucket` option, if provided in s3://bucket/key format.
	#[arg(long, value_name = "KEY")]
	pub key: Option<String>,

	/// Expiry duration of the token.
	///
	/// This is the duration for which the token will be valid. It can be specified in seconds, or
	/// with a suffix like `m` for minutes, `h` for hours, or `d` for days. The default is 12 hours.
	/// Minimum is 15 minutes and maximum is 36 hours, unless you're logged in with the root user,
	/// in which case the maximum (and default) is 1 hour. However, you should avoid using the root
	/// user for anything other than account management, which this tool is not.
	///
	/// Be careful with short expiry times! The token must remain valid for the entire duration of
	/// the upload, and if the upload takes longer than the token is valid, it will fail and will
	/// need to be retried from scratch, using a new token.
	///
	/// Longer expiries are more convenient, but also more dangerous, as they give more time for an
	/// attacker to use the token to upload files to your bucket. On the whole, though, this is a
	/// pretty hard token to misuse: the worst that can happen is someone uploading a huge file to
	/// your bucket, incurring you transfer and storage costs.
	#[arg(long, value_name = "DURATION", default_value = "12h")]
	pub expiry: humantime::Duration,

	/// Allow non-versioned buckets.
	///
	/// By default, this command will refuse to generate a token for a non-versioned bucket, because
	/// it's too easy to accidentally overwrite files. If you really want to, provide this option.
	#[arg(long)]
	pub allow_non_versioned: bool,

	/// Allow wildcard keys.
	///
	/// By default, this command will refuse to generate a token for a wildcard keys, to prevent
	/// accidents. If that's what you intended to do, provide this option. Remember to enclose the
	/// key in quotes and be careful about providing too-wide access!
	#[arg(long)]
	pub allow_wilcards: bool,

	#[command(flatten)]
	pub aws: AwsArgs,
}

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<UploadArgs, DelegateArgs>) -> Result<()> {
	let (bucket, key) = parse_bucket_and_key(&ctx.args_sub.bucket, ctx.args_sub.key.as_deref())?;

	let aws = aws::init(&ctx.args_sub.aws).await;

	if !ctx.args_sub.allow_non_versioned {
		debug!("Checking bucket is versioned");
		let client = S3Client::new(&aws);
		match client
			.get_bucket_versioning()
			.bucket(bucket)
			.send()
			.await
			.into_diagnostic().map(|r| r.status)
		{
			Ok(Some(BucketVersioningStatus::Enabled)) => (),
			Ok(Some(_)) => bail!("Bucket is not versioned, allowing delegated upload is dangerous. Use --allow-non-versioned to bypass."),
			Err(err) => bail!("Unable to check if bucket is versioned. Allowing delegated upload may be dangerous. Use --allow-non-versioned to bypass this check.\n{err:?}"),
			_ => (),
		}
	}

	if !ctx.args_sub.allow_wilcards {
		debug!("Checking key is not a wildcard");
		if key.contains('*') {
			bail!(
				"Key contains a wildcard, this can be dangerous. Use --allow-wildcards to bypass."
			);
		}
	}

	if key.ends_with('/') {
		bail!("Key ends with a slash, this can't be used to upload files. Specify the full path, or use a wildcard.");
	}

	info!(
		"Generating federated credentials to upload to s3://{}/{}",
		bucket, key
	);
	let token = DelegatedToken::new(
		&aws,
		ctx.args_sub.expiry.into(),
		&json!({
			"Version": "2012-10-17",
			"Statement": [
				{
					"Effect": "Allow",
					"Action": [
						"s3:PutObject",
						"s3:CreateMultipartUpload",
						"s3:CompleteMultipartUpload",
						"s3:AbortMultipartUpload",
						"s3:UploadPart",
					],
					"Resource": [
						format!("arn:aws:s3:::{}/{}", bucket, key),
					],
				},
			],
		}),
	)
	.await?;

	println!("{token}");
	Ok(())
}
