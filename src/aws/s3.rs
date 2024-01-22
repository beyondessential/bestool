use std::{
	borrow::Cow,
	mem::take,
	num::NonZeroU64,
	path::Path,
	sync::{
		atomic::{AtomicU32, AtomicUsize, Ordering},
		Arc,
	},
};

use aws_sdk_s3::{
	primitives::ByteStream,
	types::{builders::CompletedMultipartUploadBuilder, ChecksumAlgorithm, CompletedPart},
	Client as S3Client,
};
use miette::{bail, IntoDiagnostic, Result};
use tokio::fs::metadata;
use tracing::{debug, info, instrument};

use crate::{
	actions::{
		context::Cleanup,
		upload::{Token, UploadId},
		Context,
	},
	file_chunker::{FileChunker, DEFAULT_CHUNK_SIZE},
};

use super::MINIMUM_MULTIPART_PART_SIZE;

#[instrument(skip(ctx, client))]
pub async fn multipart_upload(
	ctx: Context,
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
	let upload_id = UploadId {
		bucket: bucket.to_string(),
		key: key.to_string(),
		id: upload_id,
		parts: chunker.chunks() as i32,
	};
	ctx.add_cleanup(Cleanup::MultiPartUpload(upload_id.clone()));

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
						.upload_id(upload_id.id)
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
				.upload_id(upload_id.id)
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
			.upload_id(upload_id.id)
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
		.upload_id(upload_id.id)
		.multipart_upload(parts.build())
		.send()
		.await
		.into_diagnostic()?;
	progress.tick();
	progress.abandon(); // finish, leaving the completed bar in place

	Ok(())
}

#[instrument(skip(ctx, token))]
pub async fn token_upload(ctx: Context, mut token: Token, file: &Path) -> Result<()> {
	debug!("Loading file {}", file.display());
	let mut chunker = FileChunker::new(file).await?;
	// UNWRAP: DEFAULT_CHUNK_SIZE is non-zero
	let token_parts = token.id.parts as u64;
	chunker.chunk_size = NonZeroU64::new(
		(chunker.len() / (token_parts - (token_parts / 10))).max(DEFAULT_CHUNK_SIZE.get()),
	)
	.unwrap();
	chunker.min_chunk_size = MINIMUM_MULTIPART_PART_SIZE;

	info!(
		chunk_size=%chunker.chunk_size,
		upload_id=?token.id,
		"Uploading {} ({} bytes) to s3://{}/{}",
		file.display(),
		chunker.len(),
		token.id.bucket,
		token.id.key
	);
	let progress = ctx.data_bar(chunker.len());
	progress.set_message(file.display().to_string());
	progress.tick();

	let parts = Arc::<[_]>::from(take(&mut token.parts).into_boxed_slice());
	let part_i = Arc::new(AtomicUsize::new(0));

	while let Some((bytes, _)) =
		match chunker
			.with_next_chunk(&{
				let upload_id = token.id.clone();
				let parts = parts.clone();
				let part_i = part_i.clone();

				move |bytes| {
					let upload_id = upload_id.clone();
					let parts = parts.clone();
					let part_i = part_i.load(Ordering::SeqCst);
					let part_no = part_i as i32 + 1;

					async move {
						let Some(part) = parts.get(part_i) else {
							bail!("Used all of the parts in the token, but still have chunks to upload!");
						};

						debug!(bytes = bytes.len(), "uploading a chunk");
						// client
						// 	.upload_part()
						// 	.body(bytes.into())
						// 	.bucket(upload_id.bucket)
						// 	.key(upload_id.key)
						// 	.checksum_algorithm(checksum)
						// 	.part_number(part_no)
						// 	.upload_id(upload_id.id)
						// 	.send()
						// 	.await
						// 	.into_diagnostic()?;

						Ok(())
					}
				}
			})
			.await
		{
			Ok(res) => res,
			Err(err) => {
				debug!(?err, "error sending chunk, stopping upload");
				return Err(err);
			}
		} {
		if part_i.fetch_add(1, Ordering::SeqCst) as u64 >= token_parts {
			bail!("Used all of the parts in the token, but still have chunks to upload!");
		}
		progress.inc(bytes);
	}

	progress.tick();
	progress.abandon(); // finish, leaving the completed bar in place

	Ok(())
}

pub async fn singlepart_upload(
	ctx: Context,
	bucket: &str,
	key: &str,
	path: &Path,
	client: &S3Client,
) -> Result<()> {
	let key = resolve_key(key, path);
	let meta = metadata(path).await.into_diagnostic()?;
	let file = ByteStream::from_path(path).await.into_diagnostic()?;

	let progress = ctx.data_bar(meta.len());
	progress.set_message(path.display().to_string());
	progress.tick();

	info!("Uploading {} to s3://{}/{}", path.display(), bucket, key);
	client
		.put_object()
		.body(file)
		.bucket(bucket)
		.key(&*key)
		.checksum_algorithm(ChecksumAlgorithm::Sha256)
		.metadata("Uploader", crate::APP_NAME)
		.send()
		.await
		.into_diagnostic()?;
	progress.inc(meta.len());
	progress.abandon(); // finish, leaving the completed bar in place

	Ok(())
}

/// Resolve a key that ends with a slash to a key that ends with the file name.
///
/// Leaves other keys unchanged.
pub fn resolve_key<'key>(key: &'key str, file: &Path) -> Cow<'key, str> {
	if key.ends_with('/') {
		let mut key = key.to_owned();
		key.push_str(file.file_name().unwrap().to_str().unwrap());
		Cow::Owned(key)
	} else {
		Cow::Borrowed(key)
	}
}

/// Parse and normalise bucket/key arguments.
///
/// If the bucket starts with `s3://`, the `s3://` is stripped off.
/// If the bucket includes a key, it is split out and returned.
pub fn parse_bucket_and_key<'a>(
	bucket: &'a str,
	key: Option<&'a str>,
) -> Result<(&'a str, &'a str)> {
	Ok(if bucket.starts_with("s3://") {
		if let Some((bucket, key)) = bucket[5..].split_once('/') {
			(bucket, key)
		} else {
			(bucket, "/")
		}
	} else if let Some(key) = key {
		(bucket, key)
	} else {
		bail!("No key specified");
	})
}
