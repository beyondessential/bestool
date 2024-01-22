use std::path::PathBuf;

use aws_sdk_s3::{presigning::PresigningConfig, types::ChecksumAlgorithm, Client as S3Client};
use clap::Parser;
use miette::{bail, IntoDiagnostic, Result};
use tokio::{fs::File, io::AsyncWriteExt};
use tracing::{debug, info, instrument};

use crate::{
	actions::{
		upload::token::{encode_token, UploadId},
		Context,
	},
	aws::{self, s3::parse_bucket_and_key, MINIMUM_MULTIPART_PART_SIZE},
	file_chunker::DEFAULT_CHUNK_SIZE,
};

use super::UploadArgs;

/// Generate a pre-auth token to upload a file.
///
/// This MUST be run from your local, trusted computer, using WriteAccess or AdminAccess to the
/// account that contains the desired destination bucket. It will generate a pre-auth token that
/// allows anyone to upload a file to the specified bucket and key, without them needing any AWS
/// credentials.
///
/// As a token can be quite large, it will be written to a file, by default `token.txt`. Override
/// this with `--token-file`. The contents of the file are text, so can be copy-pasted rather than
/// requiring to transmit the file verbatim.
///
/// Once all parts are uploaded, the upload must be confirmed using `bestool upload confirm`. Take
/// note of the "Upload ID" printed by this command, as it will be needed to confirm the upload, to
/// see its status with `bestool upload status`, or to cancel it with `bestool upload cancel`.
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
/// Tokens also have an expiry date. By default, tokens expire after 2 HOURS. You can specify a
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
	/// with a suffix like `m` for minutes, `h` for hours, or `d` for days. The default is 2 hours.
	///
	/// Be careful with short expiry times! The token must remain valid for the entire duration of
	/// the upload, and if the upload takes longer than the token is valid, it will fail and will
	/// need to be retried from scratch, using a new token. You can't issue a token valid for longer
	/// than your own credentials are; this is mostly an issue when using temporary (eg SSO) creds.
	///
	/// Longer expiries are more convenient, but also more dangerous, as they give more time for an
	/// attacker to use the token to upload files to your bucket. On the whole, though, this is a
	/// pretty hard token to misuse: the worst that can happen is someone uploading a huge file to
	/// your bucket, incurring you transfer and storage costs.
	#[arg(long, value_name = "DURATION", default_value = "2h")]
	pub expiry: humantime::Duration,

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
	///
	/// The absolute maximum is 10000.
	#[arg(
		long,
		value_name = "PARTS",
		value_parser = clap::value_parser!(u64).range(1..10000),
		required_unless_present = "approximate_size",
	)]
	pub max_parts: Option<u64>,

	/// Approximate size of the entire file.
	///
	/// Provide the approximate size of the entire file, in bytes. You can use suffixes like `K`,
	/// `M`, or `G`. It will be used to calculate the maximum number of parts the file can be split
	/// into, including some extra parts for retries.
	#[arg(long, value_name = "SIZE", required_unless_present = "max_parts")]
	pub approximate_size: Option<bytesize::ByteSize>,

	/// File to save the token to.
	#[arg(long, value_name = "FILENAME", default_value = "token.txt")]
	pub token_file: PathBuf,

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

#[instrument(skip(ctx))]
pub async fn run(ctx: Context<UploadArgs, PreauthArgs>) -> Result<()> {
	let (bucket, key) = parse_bucket_and_key(&ctx.args_sub.bucket, ctx.args_sub.key.as_deref())?;

	let parts = i32::try_from(if let Some(max_parts) = ctx.args_sub.max_parts {
		max_parts
	} else if let Some(approximate_size) = ctx.args_sub.approximate_size {
		let approximate_size = approximate_size.as_u64();
		if approximate_size == 0 {
			bail!("--approximate-size cannot be zero");
		}

		let small_chunks = plus_margin(approximate_size / MINIMUM_MULTIPART_PART_SIZE.get());
		let medium_chunks = plus_margin(approximate_size / DEFAULT_CHUNK_SIZE.get());
		let large_chunks = plus_margin(approximate_size / DEFAULT_CHUNK_SIZE.get());

		if small_chunks <= 1000 {
			small_chunks
		} else if medium_chunks <= 1000 {
			medium_chunks
		} else if large_chunks <= 1000 {
			large_chunks
		} else {
			1000
		}
	} else {
		unreachable!("clap should enforce one of max_parts or approximate_size");
	})
	.into_diagnostic()?;

	if ctx.args_sub.token_file.exists() {
		bail!(
			"Token file {:?} already exists, not overwriting",
			ctx.args_sub.token_file
		);
	}

	info!(
		"Generating pre-auth token for s3://{}/{} ({} parts)",
		bucket, key, parts
	);
	let aws = aws::init(&ctx.args_sub).await;
	let client = S3Client::new(&aws);

	let progress = ctx.bar((parts as u64) + 3);
	progress.tick();

	debug!("Creating multipart upload");
	progress.set_message("create multipart");
	let checksum = ChecksumAlgorithm::Sha256;
	let mp = client
		.create_multipart_upload()
		.bucket(bucket)
		.key(&*key)
		.checksum_algorithm(checksum.clone())
		.metadata("Preauther", crate::APP_NAME)
		.send()
		.await
		.into_diagnostic()?;
	progress.inc(1);

	let Some(upload_id) = mp.upload_id else {
		bail!("No upload ID returned from S3");
	};
	let upload_id = UploadId {
		bucket: bucket.into(),
		key: key.into(),
		id: upload_id,
		parts,
	};

	ctx.progress.suspend(|| eprintln!("Upload ID: {upload_id}"));

	let presigning = PresigningConfig::expires_in(ctx.args_sub.expiry.into()).into_diagnostic()?;
	info!(
		"Pre-auth token valid from {:?}, expires in {:?}",
		presigning.start_time(),
		presigning.expires()
	);

	progress.set_message("presign multipart");
	let mut presigned_parts = Vec::with_capacity(parts as usize);
	for part_no in 1..=parts {
		progress.inc(1);
		presigned_parts.push(
			client
				.upload_part()
				.bucket(bucket)
				.key(key)
				.checksum_algorithm(checksum.clone())
				.part_number(part_no)
				.upload_id(&upload_id.id)
				.presigned(presigning.clone())
				.await
				.into_diagnostic()?,
		);
	}

	progress.set_message("generate token");
	let token = encode_token(&upload_id, &presigned_parts)?;
	progress.inc(1);

	progress.set_message(format!("write to {}", ctx.args_sub.token_file.display()));
	let mut file = File::create(&ctx.args_sub.token_file)
		.await
		.into_diagnostic()?;
	file.write_all(token.as_bytes()).await.into_diagnostic()?;
	progress.inc(1);

	progress.abandon();
	Ok(())
}

fn plus_margin(n: u64) -> u64 {
	n.max(1) + (n / 10).max(9)
}
