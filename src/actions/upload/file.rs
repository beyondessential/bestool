use std::path::PathBuf;

use clap::{Parser, ValueHint};
use miette::{bail, Result};

use super::UploadArgs;

/// Upload a file to AWS S3.
///
/// There's two ways to upload a file: using AWS credentials, or using a pre-auth token. If you have
/// AWS credentials, you can use the `--bucket` and `--key` options to specify the bucket and key to
/// upload to. If you don't have AWS credentials, you can use the `--pre-auth` option to specify a
/// pre-auth token generated by `bestool upload preauth`, either by you on a trusted computer, or by
/// someone else who has AWS credentials with write access to a bucket.
///
/// If you use the `--pre-auth` option, the token generator specifies the number of parts the file
/// can be split into. In general, that will be enough to upload the file in small chunks and retry
/// any failed chunks, which is more reliable than uploading the file in one go, especially for very
/// big files and flaky connections. If you use AWS credentials and have the required permissions,
/// the tool will adaptively split the file into chunks and retry failed chunks, without the limits
/// inherent from the pre-auth token. That often leads to faster uploads than with a pre-auth token.
///
/// If you need to upload an entire folder, either archive it first (e.g. with Zip or tar), or use
/// `bestool upload folder` instead, which archives the folder itself and otherwise behaves as here.
///
/// Uploading multiple files to the same bucket and key is possible (specify multiple PATHS here),
/// but only with AWS credentials, not with a pre-auth token.
#[derive(Debug, Clone, Parser)]
pub struct FileArgs {
	/// File(s) to upload.
	///
	/// You can specify multiple files here, and they will all be uploaded to the same bucket and
	/// key, which must in this case end with a slash. Uploading multiple files is not possible with
	/// a pre-auth token.
	#[arg(
		value_hint = ValueHint::FilePath,
		value_name = "PATH",
		required = true,
	)]
	pub files: Vec<PathBuf>,

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

	/// Pre-auth token to use.
	///
	/// This is a pre-auth token generated by `bestool upload preauth`. Setting this is exclusive to
	/// setting `--bucket` and `--key`, as the pre-auth token is generated for a particular bucket
	/// and filename within that bucket.
	///
	/// Using this option, you can upload a file without AWS credentials. Note that pre-auth tokens
	/// are time-limited: if you try to use an expired token, the upload will fail; more critically,
	/// if the upload takes longer than the token's lifetime, the upload will also fail.
	#[arg(long, value_name = "TOKEN", conflicts_with_all = &["bucket", "key", "aws_*"])]
	pub pre_auth: Option<String>,

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

pub async fn run(_args: UploadArgs, mut subargs: FileArgs) -> Result<()> {
	if let Some(token) = subargs.pre_auth {
		if subargs.files.len() > 1 {
			bail!("Cannot upload multiple files with a pre-auth token");
		}

		let Some(file) = subargs.files.pop() else {
			bail!("No file to upload");
		};

		with_preauth(token, file).await
	} else {
		with_aws().await
	}
}

pub async fn with_preauth(_token: String, _file: PathBuf) -> Result<()> {
	Ok(())
}

pub async fn with_aws() -> Result<()> {
	Ok(())
}
