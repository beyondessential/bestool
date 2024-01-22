use clap::Parser;
use miette::Result;

use crate::actions::Context;

use super::UploadArgs;

/// Generate a pre-auth token to upload a file.
///
/// This MUST be run from your local, trusted computer, using WriteAccess or AdminAccess to the
/// account that contains the desired destination bucket. It will generate a pre-auth token that
/// allows anyone to upload a file to the specified bucket and key, without them needing any AWS
/// credentials.
///
/// When creating a token, you specify the maximum number of parts the file can be split into,
/// either explicitly with `--max-parts` or by giving `--approximate-size`, which will use internal
/// logic to produce an appropriate number for most situations. This will be used to upload the file
/// in small chunks and retry any failed chunks, which is more reliable than uploading the file in
/// one go, especially for very large files and flaky connections.
///
/// If the uploader runs out of parts to use before the file is fully uploaded, it will fail and a
/// new token will have to be generated with a higher number of parts.
///
/// Tokens also have an expiry date. By default, tokens expire after 6 HOURS. You can specify a
/// longer or shorter expiry time with `--expiry`. Be careful with short expiry times! The token
/// must remain valid for the entire duration of the upload, and if the upload takes longer than the
/// token is valid, it will fail and will need to be retried from scratch, using a new token.
///
/// A maximum of 1000 tokens can be active at any one time for a particular bucket.
#[derive(Debug, Clone, Parser)]
pub struct PreauthArgs {
	/// AWS S3 bucket to upload to.
	///
	/// This may also contain the key, if given in s3://bucket/key format. See the `--key` option
	/// for semantics of the key portion.
	#[arg(long, value_name = "BUCKET", required = true)]
	pub bucket: String,

	/// Pathname in the bucket to upload to.
	///
	/// If not specified, the file will be uploaded to the root of the bucket.
	///
	/// If this ends with a slash, the file will be uploaded to a directory, and the filename will
	/// be the same as the local filename. If this does not end with a slash, the file will be given
	/// the exact name as specified here.
	///
	/// You can also give the key via the `--bucket` option, if provided in s3://bucket/key format.
	#[arg(long, value_name = "KEY")]
	pub key: Option<String>,

	/// Expiry duration of the token.
	///
	/// This is the duration for which the token will be valid. It can be specified in seconds, or
	/// with a suffix like `m` for minutes, `h` for hours, or `d` for days. The default is 6 hours.
	///
	/// Be careful with short expiry times! The token must remain valid for the entire duration of
	/// the upload, and if the upload takes longer than the token is valid, it will fail and will
	/// need to be retried from scratch, using a new token.
	///
	/// Longer expiries are more convenient, but also more dangerous, as they give more time for an
	/// attacker to use the token to upload files to your bucket. In general, stick with a few hours
	/// for use on a call, or a few days for giving to someone else. Automated use with small files
	/// on reliable connections can use shorter expiries.
	#[arg(long, value_name = "DURATION", default_value = "6h")]
	pub expiry: String,

	/// Maximum number of parts the file can be split into.
	///
	/// This is the maximum number of parts the file can be split into. If not specified, it will be
	/// calculated from `--approximate-size`, using internal logic to produce an appropriate number.
	///
	/// If you specify both, `--max-parts` takes precedence.
	///
	/// If the uploader runs out of parts to use before the file is fully uploaded, it will fail and
	/// a new token will have to be generated with a higher number of parts; for this reason you
	/// should include some extra parts in the number you specify here if calculating it from a size
	/// yourself; 10% extra is a good rule of thumb.
	#[arg(long, value_name = "PARTS")]
	pub max_parts: Option<usize>,

	/// Approximate size of the entire file.
	///
	/// This is the approximate size of the entire file, in bytes. You can use suffixes like `K`,
	/// `M`, or `G`. It will be used to calculate the maximum number of parts the file can be split
	/// into, including some extra parts for retries.
	#[arg(long, value_name = "SIZE")]
	pub approximate_size: Option<String>,

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

crate::aws::standard_aws_args!(PreauthArgs);

pub async fn run(_ctx: Context<UploadArgs, PreauthArgs>) -> Result<()> {
	Ok(())
}
