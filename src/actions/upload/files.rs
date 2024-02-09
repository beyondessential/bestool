use std::{mem::take, path::PathBuf};

use aws_sdk_s3::Client as S3Client;
use clap::{Parser, ValueHint};
use miette::{bail, Result};
use tracing::{error, instrument, warn};

use crate::{
	actions::Context,
	aws::{
		self,
		s3::{multipart_upload, parse_bucket_and_key, singlepart_upload},
		AwsArgs,
	},
};

use super::UploadArgs;

/// Upload files to AWS S3.
///
/// There's two ways to upload a file: using AWS credentials, or using a delegated token. If you
/// don't have AWS credentials on the machine with the file you want to upload, you or someone with
/// the rights to can use the `--aws-delegated` option to specify a delegated token generated by
/// `bestool upload delegate`.
///
/// This tool uploads files in small chunks and retries any failed chunks, which is more reliable
/// than uploading the file in one go, especially for very big files and flaky connections. It will
/// adaptively reduce the chunking size as it retries failed chunks, to work around broken networks.
///
/// If you need to upload an entire folder, use `bestool archive` to generate an archive file,
/// optionally with compression, and upload that instead. Alternatively, you can upload multiple
/// files to the same bucket and key (specify multiple PATH arguments), and even use `--recursive`.
#[derive(Debug, Clone, Parser)]
pub struct FilesArgs {
	/// File(s) to upload.
	///
	/// You can specify multiple files here, and they will all be uploaded to the same bucket and
	/// key, which must in this case end with a slash. Also see `--recursive` for the behaviour when
	/// uploading folders.
	#[arg(
		value_hint = ValueHint::FilePath,
		value_name = "PATH",
		required = true,
		num_args = 1..,
	)]
	pub files: Vec<PathBuf>,

	/// AWS S3 bucket to upload to.
	///
	/// This may also contain the key, if given in s3://bucket/key format. See the `--key` option
	/// for semantics of the key portion.
	///
	/// If using a delegated token, this bucket must match the bucket the token was generated for.
	#[arg(long, value_name = "BUCKET")]
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
	///
	/// If using a delegated token, this key must match the key the token was generated for.
	#[arg(long, value_name = "KEY")]
	pub key: Option<String>,

	/// Recurse into folders.
	///
	/// Any folders given in the PATH arguments will be listed, all files found will be uploaded,
	/// and any subfolders found will be recursed into.
	///
	/// If this is not specified, any folders given in the PATH arguments will error.
	#[arg(long)]
	pub recursive: bool,

	#[command(flatten)]
	pub aws: AwsArgs,
}

#[instrument(skip(ctx))]
pub async fn run(mut ctx: Context<UploadArgs, FilesArgs>) -> Result<()> {
	let (bucket, key) = parse_bucket_and_key(&ctx.args_sub.bucket, ctx.args_sub.key.as_deref())?;
	let aws = aws::init(&ctx.args_sub.aws).await;
	let client = S3Client::new(&aws);

	let files = take(&mut ctx.args_sub.files);
	let files = if ctx.args_sub.recursive {
		let mut filtered = Vec::new();
		for file in files {
			if file.is_dir() {
				filtered.extend(
					walkdir::WalkDir::new(file)
						.into_iter()
						.filter_map(|entry| entry.ok())
						.filter(|entry| entry.file_type().is_file())
						.map(|entry| entry.into_path()),
				);
			} else {
				filtered.push(file);
			}
		}
		filtered
	} else {
		for file in &files {
			if file.is_dir() {
				bail!("Cannot upload a directory without --recursive",);
			}
		}
		files
	};

	let (first, files) = {
		let (left, right) = files.split_at(1);
		let Some(left) = left.get(0) else {
			bail!("No files to upload");
		};

		(left, right)
	};

	let use_multipart =
		if let Err(err) = multipart_upload(ctx.erased(), bucket, key, first, &client).await {
			error!(?err, "Upload failed with multipart");
			warn!("Attempting single-part upload(s) instead");
			singlepart_upload(ctx.erased(), bucket, key, first, &client).await?;
			false
		} else {
			true
		};

	for file in files {
		if use_multipart {
			multipart_upload(ctx.erased(), bucket, key, file, &client).await?;
		} else {
			singlepart_upload(ctx.erased(), bucket, key, file, &client).await?;
		}
	}

	Ok(())
}