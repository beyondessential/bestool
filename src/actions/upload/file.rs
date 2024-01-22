use std::{
	borrow::Cow,
	num::NonZeroU64,
	path::{Path, PathBuf},
	sync::{
		atomic::{AtomicU32, Ordering},
		Arc,
	},
};

use aws_sdk_s3::{
	types::{builders::CompletedMultipartUploadBuilder, ChecksumAlgorithm, CompletedPart},
	Client as S3Client,
};
use clap::{Parser, ValueHint};
use miette::{bail, IntoDiagnostic, Result};
use tracing::{debug, error, info, instrument, warn};

use crate::{
	actions::Context,
	aws::{self, MINIMUM_MULTIPART_PART_SIZE},
	file_chunker::{FileChunker, DEFAULT_CHUNK_SIZE},
};

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
	#[arg(long, value_name = "BUCKET", required_unless_present = "pre_auth")]
	pub bucket: Option<String>,

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
	#[arg(long, value_name = "TOKEN", conflicts_with_all = &["bucket", "key", "aws_access_key_id", "aws_secret_access_key", "aws_region"])]
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

crate::aws::standard_aws_args!(FileArgs);

pub async fn run(mut ctx: Context<UploadArgs, FileArgs>) -> Result<()> {
	if let Some(token) = ctx.args_sub.pre_auth {
		if ctx.args_sub.files.len() > 1 {
			bail!("Cannot upload multiple files with a pre-auth token");
		}

		let Some(file) = ctx.args_sub.files.pop() else {
			bail!("No file to upload");
		};

		with_preauth(token, file).await
	} else if let Some(bucket) = ctx.args_sub.bucket.as_deref() {
		let (bucket, key) = if bucket.starts_with("s3://") {
			if let Some((bucket, key)) = bucket[5..].split_once('/') {
				(bucket, key)
			} else {
				(bucket, "/")
			}
		} else if let Some(key) = ctx.args_sub.key.as_deref() {
			(bucket, key)
		} else {
			bail!("No key specified");
		};

		with_aws(ctx.clone(), bucket, key).await
	} else {
		bail!("No bucket or pre-auth token specified");
	}
}

pub async fn with_preauth(_token: String, _file: PathBuf) -> Result<()> {
	Ok(())
}

pub async fn with_aws(ctx: Context<UploadArgs, FileArgs>, bucket: &str, key: &str) -> Result<()> {
	let aws = aws::init(&ctx.args_sub).await;
	let client = S3Client::new(&aws);

	let (first, files) = {
		let (left, right) = ctx.args_sub.files.split_at(1);
		// UNWRAP: length is checked to >= 1 in run()
		(left.get(0).unwrap(), right)
	};

	let use_multipart =
		if let Err(err) = multipart_upload(ctx.clone(), bucket, key, first, &client).await {
			error!(?err, "Upload failed with multipart");
			warn!("Attempting single-part upload(s) instead");
			singlepart_upload(ctx.clone(), bucket, key, first, &client).await?;
			false
		} else {
			true
		};

	for file in files {
		if use_multipart {
			multipart_upload(ctx.clone(), bucket, key, file, &client).await?;
		} else {
			singlepart_upload(ctx.clone(), bucket, key, file, &client).await?;
		}
	}

	Ok(())
}

#[instrument(skip(ctx, client))]
async fn multipart_upload(
	ctx: Context<UploadArgs, FileArgs>,
	bucket: &str,
	key: &str,
	file: &Path,
	client: &S3Client,
) -> Result<()> {
	let key = resolve_key(key, file);

	debug!("Loading file {}", file.display());
	let mut chunker = FileChunker::new(file).await?;
	// UNWRAP: DEFAULT_CHUNK_SIZE is non-zero
	chunker.chunk_size =
		NonZeroU64::new((chunker.len() / 1_000).max(DEFAULT_CHUNK_SIZE.get())).unwrap();
	chunker.min_chunk_size = MINIMUM_MULTIPART_PART_SIZE;

	debug!(chunk_size = chunker.chunk_size, "Creating multipart upload");
	let checksum = ChecksumAlgorithm::Sha256;
	let mp = client
		.create_multipart_upload()
		.bucket(bucket)
		.key(&*key)
		.checksum_algorithm(checksum.clone())
		.metadata("Uploader", crate::APP_NAME)
		.send()
		.await
		.into_diagnostic()?;

	let Some(upload_id) = mp.upload_id else {
		bail!("No upload ID returned from S3");
	};

	info!(
		"Uploading {} ({} bytes) to s3://{}/{}",
		file.display(),
		chunker.len(),
		bucket,
		key
	);
	let progress = ctx.data_bar(chunker.len());
	progress.set_message(file.display().to_string());
	progress.tick();

	let mut parts = CompletedMultipartUploadBuilder::default();
	let part_no = Arc::new(AtomicU32::new(1));

	while let Some((bytes, part)) = match chunker
		.with_next_chunk(&{
			let client = client.clone();
			let bucket = bucket.to_string();
			let key = key.to_string();
			let checksum = checksum.clone();
			let upload_id = upload_id.clone();
			let part_no = part_no.clone();

			move |bytes| {
				let client = client.clone();
				let bucket = bucket.clone();
				let key = key.clone();
				let checksum = checksum.clone();
				let upload_id = upload_id.clone();
				let part_no = part_no.load(Ordering::SeqCst) as i32;
				async move {
					debug!(bytes = bytes.len(), "uploading a chunk");
					let upload = client
						.upload_part()
						.body(bytes.into())
						.bucket(bucket)
						.key(key)
						.checksum_algorithm(checksum)
						.part_number(part_no)
						.upload_id(upload_id)
						.send()
						.await
						.into_diagnostic()?;

					Ok(CompletedPart::builder()
						.set_e_tag(upload.e_tag)
						.set_checksum_crc32(upload.checksum_crc32)
						.set_checksum_crc32_c(upload.checksum_crc32_c)
						.set_checksum_sha1(upload.checksum_sha1)
						.set_checksum_sha256(upload.checksum_sha256)
						.part_number(part_no)
						.build())
				}
			}
		})
		.await
	{
		Ok(res) => res,
		Err(err) => {
			debug!(?err, "error sending chunk, aborting multipart upload");
			client
				.abort_multipart_upload()
				.bucket(bucket)
				.key(&*key)
				.upload_id(upload_id)
				.send()
				.await
				.into_diagnostic()?;
			return Err(err);
		}
	} {
		part_no.fetch_add(1, Ordering::SeqCst);
		parts = parts.parts(part);
		progress.inc(bytes);
	}

	if chunker.chunks() == 0 {
		debug!("no chunks read, cancel multipart upload");
		client
			.abort_multipart_upload()
			.bucket(bucket)
			.key(&*key)
			.upload_id(upload_id)
			.send()
			.await
			.into_diagnostic()?;

		bail!("No chunks read from file (unexpected)!");
	}

	debug!(?parts, "finalise multipart upload");
	client
		.complete_multipart_upload()
		.bucket(bucket)
		.key(&*key)
		.upload_id(upload_id)
		.multipart_upload(parts.build())
		.send()
		.await
		.into_diagnostic()?;
	progress.tick();
	progress.abandon(); // finish, leaving the completed bar in place

	Ok(())
}

async fn singlepart_upload(
	_ctx: Context<UploadArgs, FileArgs>,
	bucket: &str,
	key: &str,
	file: &Path,
	_client: &S3Client,
) -> Result<()> {
	let key = resolve_key(key, file);

	info!("Uploading {} to s3://{}/{}", file.display(), bucket, key);
	todo!("singlepart upload")
}

fn resolve_key<'key>(key: &'key str, file: &Path) -> Cow<'key, str> {
	if key.ends_with('/') {
		let mut key = key.to_owned();
		key.push_str(file.file_name().unwrap().to_str().unwrap());
		Cow::Owned(key)
	} else {
		Cow::Borrowed(key)
	}
}
