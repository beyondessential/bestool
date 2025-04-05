use std::{
	collections::{BTreeMap, HashMap},
	sync::{Arc, LazyLock},
	time::UNIX_EPOCH,
};

use aws_lambda_events::event::s3::S3Event;
use aws_sdk_s3::types::CompletedPart;
use lambda_runtime::{LambdaEvent, tracing};
use miette::{IntoDiagnostic, Result};
use serde::{Deserialize, Serialize};
use tokio::{sync::Mutex, task::JoinSet};

static CHUNK_META_CACHE: LazyLock<Mutex<HashMap<String, ChunkVersion>>> =
	LazyLock::new(|| Mutex::new(HashMap::new()));

//  1. Extract file info from payload
//  2. Filter to write events to inbox
//  3. Work out the chunked file
//  4. Check the read/write locks -> if existing and not expired, bail
//  5. Take a read lock = in state/nameoffile/read -> expiry = start of lambda + 900
//  6. Check integrity checked flag, if set and valid go to (9)
//  7. Verify integrity of chunks and whole file etc
//  8. Write integrity flag = in state/nameoffile/integrity -> list of S3 file version IDs for the whole folder
//  9. Take a write lock = in state/nameoffile/write -> expiry = start of lambda + 900
// 10. Create a multipart upload to outbox/nameoffile
// 11. Stream each chunk from S3 and back to S3 as a part of the multipart upload
//     - possibly do them in parallel, though we can't do whole-file checksumming then
//     - but that's probably fine, we've done a full integrity check before starting the write
// 12. Finalise the upload
// 13. Wipe the prefix from the inbox
// 14. Wipe the prefix from the state (locks/flags)
// 15. Notify a SQS topic?

pub(crate) async fn function_handler(event: LambdaEvent<S3Event>) -> Result<()> {
	let config = aws_config::load_from_env().await;
	let s3_client = aws_sdk_s3::Client::new(&config);
	let expiry_ts = event
		.context
		.deadline()
		.duration_since(UNIX_EPOCH)
		.into_diagnostic()?
		.as_secs();

	let payload = event.payload;
	tracing::info!("Payload: {:?}", payload);

	let mut tasks = JoinSet::new();
	for record in payload.records {
		if record
			.event_name
			.as_deref()
			.is_some_and(|name| name.starts_with("ObjectCreated:"))
		{
			continue;
		}
		let Some(bucket) = record.s3.bucket.name else {
			continue;
		};
		let Some(key) = record.s3.object.key else {
			continue;
		};
		let Some(filekey) = key.strip_prefix("inbox/") else {
			continue;
		};
		let Some((filename, _)) = filekey.split_once('/') else {
			continue;
		};

		let file = ChunkedFile::new(s3_client.clone(), bucket, expiry_ts, filename);
		tasks.spawn(async move {
			let mut file = file;

			if !file.acquire_lock(LockType::Read).await? {
				tracing::info!(?file.name, "File is read-locked, skipping");
				return Ok(());
			}

			if !file.check_integrity().await? {
				tracing::error!(?file.name, "File is corrupted or not yet complete, skipping");
				return Ok(());
			}

			if !file.acquire_lock(LockType::Write).await? {
				tracing::error!(?file.name, "File is write-locked even though we've got the read-lock, something is seriously wrong, bailing");
				return Ok(());
			}

			file.concat().await?;
			file.cleanup().await?;

			Result::Ok(())
		});
	}

	let mut errored = false;
	while let Some(res) = tasks.join_next().await {
		if let Err(err) = res.into_diagnostic().and_then(|x| x) {
			tracing::error!("Task error: {err}");
			errored = true;
		}
	}

	if errored {
		return Err(miette::miette!("One or more tasks failed"));
	}

	Ok(())
}
#[derive(Debug)]
struct ChunkedFile {
	s3: aws_sdk_s3::Client,
	bucket: String,
	expiry_ts: u64,
	name: String,
	metadata: Option<Arc<ChunkedMetadata>>,
}

impl ChunkedFile {
	fn new(s3: aws_sdk_s3::Client, bucket: String, expiry_ts: u64, filename: &str) -> Self {
		Self {
			s3,
			bucket,
			expiry_ts,
			name: filename.into(),
			metadata: None,
		}
	}

	async fn read_metadata(&mut self) -> Result<Arc<ChunkedMetadata>> {
		if self.metadata.is_none() {
			let metadata = ChunkedMetadata::read(
				&self.s3,
				&self.bucket,
				&self.inbox_filename("metadata.json"),
			)
			.await?;

			self.metadata = Some(Arc::new(metadata));
		}

		Ok(Arc::clone(self.metadata.as_ref().unwrap()))
	}

	fn lock_filename(&self, lock_type: LockType) -> String {
		match lock_type {
			LockType::Read => format!("state/{}/readlock", self.name),
			LockType::Write => format!("state/{}/writelock", self.name),
		}
	}

	fn integrity_flag(&self) -> String {
		format!("state/{}/integrity", self.name)
	}

	fn inbox_filename(&self, file: &str) -> String {
		format!("inbox/{}/{file}", self.name)
	}

	fn lock_body(&self) -> String {
		serde_json::json!({ "expiry": self.expiry_ts }).to_string()
	}

	async fn acquire_lock(&self, lock_type: LockType) -> Result<bool> {
		let lock_filename = self.lock_filename(lock_type);
		let put_result = self
			.s3
			.put_object()
			.body(self.lock_body().into_bytes().into())
			.bucket(&self.bucket)
			.key(&lock_filename)
			.if_none_match("*")
			.send()
			.await
			.map_err(|err| err.into_service_error());

		match put_result {
			Ok(_) => Ok(true),
			Err(err) => {
				if err.meta().code() == Some("419") {
					// Lock exists, check if expired
					let existing_lock = self
						.s3
						.get_object()
						.bucket(&self.bucket)
						.key(&lock_filename)
						.send()
						.await
						.map_err(|err| err.into_service_error())
						.into_diagnostic()?;

					let etag = existing_lock
						.e_tag()
						.ok_or_else(|| miette::miette!("Missing etag"))?
						.to_owned();
					let content = existing_lock.body.collect().await.into_diagnostic()?;
					let lock_data: serde_json::Value =
						serde_json::from_slice(&content.to_vec()).into_diagnostic()?;
					let lock_expiry = lock_data["expiry"]
						.as_u64()
						.ok_or_else(|| miette::miette!("Invalid lock expiry"))?;

					let current_time = std::time::SystemTime::now()
						.duration_since(UNIX_EPOCH)
						.into_diagnostic()?
						.as_secs();

					if current_time > lock_expiry {
						// Lock is expired, try to acquire with etag check
						self.s3
							.put_object()
							.body(self.lock_body().into_bytes().into())
							.bucket(&self.bucket)
							.key(&lock_filename)
							.if_match(etag)
							.send()
							.await
							.map_err(|err| err.into_service_error())
							.into_diagnostic()?;
						Ok(true)
					} else {
						Ok(false)
					}
				} else {
					Err(err).into_diagnostic()
				}
			}
		}
	}

	async fn check_integrity(&mut self) -> Result<bool> {
		// if integrity file exists, check that it still tracks the latest versions of chunks
		if let Some(resp) = self
			.s3
			.get_object()
			.bucket(&self.bucket)
			.key(self.integrity_flag())
			.send()
			.await
			.map_err(|err| err.into_service_error())
			.map(Some)
			.or_else(|err| {
				if err.meta().code() == Some("404") {
					Ok(None)
				} else {
					Err(err)
				}
			})
			.into_diagnostic()?
		{
			let content = resp.body.collect().await.into_diagnostic()?;
			let map: HashMap<String, String> =
				serde_json::from_slice(&content.to_vec()).into_diagnostic()?;
			let mut invalid = false;

			for (chunk, version_id) in map {
				match self.chunk_meta(&chunk).await? {
					None => {
						tracing::warn!(?chunk, "Chunk is missing");
						invalid = true;
					}
					Some(meta) if meta.0 != version_id => {
						tracing::warn!(
							?chunk,
							"Chunk has been overwritten since integrity was checked",
						);
						invalid = true;
					}
					_ => { /* we're good */ }
				}
			}

			if !invalid {
				return Ok(true);
			}
		}

		let meta = self.read_metadata().await?;

		let list_resp = self
			.s3
			.list_objects_v2()
			.bucket(&self.bucket)
			.prefix(self.inbox_filename(""))
			.send()
			.await
			.into_diagnostic()?;

		let chunk_count = list_resp
			.contents()
			.iter()
			.filter(|obj| obj.key().is_some_and(|k| k.ends_with(".chunk")))
			.count();

		if chunk_count as u64 != meta.chunk_n {
			tracing::error!(
				"Metadata chunk_n doesn't match reality: expected {}, got {}",
				meta.chunk_n,
				chunk_count
			);
			return Ok(false);
		}

		if meta.chunk_n != meta.chunks.len() as u64 {
			tracing::error!(
				"Metadata file is self-inconsistent: chunk_n != chunks.len(): {} != {}",
				meta.chunk_n,
				meta.chunks.len()
			);
			return Ok(false);
		}

		let expected_hashes: BTreeMap<String, blake3::Hash> = meta
			.chunks
			.iter()
			.map(|(k, v)| {
				blake3::Hash::from_hex(v)
					.map(|hash| (k.clone(), hash))
					.map_err(|e| miette::miette!("Invalid hash hex in metadata: {}", e))
			})
			.collect::<Result<_>>()?;

		let mut whole_hasher = blake3::Hasher::new();
		let mut whole_size = 0;
		let mut chunk_version_ids = HashMap::new();

		for (n, (chunk, expected_hash)) in expected_hashes.iter().enumerate() {
			let chunk_object = self
				.s3
				.get_object()
				.bucket(&self.bucket)
				.key(self.inbox_filename(chunk))
				.send()
				.await
				.into_diagnostic()?;

			if let Some(version_id) = chunk_object.version_id() {
				chunk_version_ids.insert(chunk.clone(), version_id.to_string());
			}

			let mut hasher = blake3::Hasher::new();
			let mut size = 0;
			let mut stream = chunk_object.body;

			while let Some(chunk) = stream.try_next().await.into_diagnostic()? {
				hasher.update(&chunk);
				whole_hasher.update(&chunk);
				size += chunk.len();
				whole_size += chunk.len();
			}

			if n == meta.chunks.len() - 1 {
				if size as u64 > meta.chunk_size {
					tracing::error!(
						"Final chunk size {} is larger than chunk_size {}",
						size,
						meta.chunk_size
					);
					return Ok(false);
				}
			} else if size as u64 != meta.chunk_size {
				tracing::error!(
					"Chunk {} size {} does not match chunk_size {}",
					chunk,
					size,
					meta.chunk_size
				);
				return Ok(false);
			}

			let actual_hash = hasher.finalize();
			if actual_hash != *expected_hash {
				tracing::error!(
					"Chunk hash mismatch for {}: expected {}, got {}",
					chunk,
					expected_hash,
					actual_hash,
				);
				return Ok(false);
			}
		}

		let actual_whole_hash = whole_hasher.finalize();
		let expected_whole_hash = blake3::Hash::from_hex(&meta.full_sum)
			.map_err(|e| miette::miette!("Invalid hash hex in metadata: {}", e))?;

		if actual_whole_hash != expected_whole_hash {
			tracing::error!(
				"Full hash mismatch: expected {}, got {}",
				expected_whole_hash,
				actual_whole_hash,
			);
			return Ok(false);
		}

		if whole_size as u64 != meta.full_size {
			tracing::error!(
				"Full size mismatch: expected {}, got {}",
				meta.full_size,
				whole_size
			);
			return Ok(false);
		}

		self.s3
			.put_object()
			.body(
				serde_json::to_string(&chunk_version_ids)
					.into_diagnostic()?
					.into_bytes()
					.into(),
			)
			.bucket(&self.bucket)
			.key(self.integrity_flag())
			.send()
			.await
			.map_err(|err| err.into_service_error())
			.into_diagnostic()?;

		Ok(true)
	}

	async fn chunk_meta(&self, file: &str) -> Result<Option<ChunkVersion>> {
		ChunkVersion::fetch_from_s3(&self.s3, &self.bucket, &self.inbox_filename(file)).await
	}

	async fn upload_part(
		s3: &aws_sdk_s3::Client,
		bucket: &str,
		inbox_path: &str,
		outbox_path: &str,
		upload_id: &str,
		part_number: i32,
	) -> Result<CompletedPart> {
		let chunk_resp = s3
			.get_object()
			.bucket(bucket)
			.key(inbox_path)
			.send()
			.await
			.map_err(|err| err.into_service_error())
			.into_diagnostic()?;

		let part = s3
			.upload_part()
			.bucket(bucket)
			.key(outbox_path)
			.upload_id(upload_id)
			.body(chunk_resp.body)
			.part_number(part_number)
			.send()
			.await
			.map_err(|err| err.into_service_error())
			.into_diagnostic()?;

		Ok(CompletedPart::builder()
			.e_tag(part.e_tag.unwrap())
			.part_number(part_number)
			.build())
	}

	async fn concat(&mut self) -> Result<()> {
		let meta = self.read_metadata().await?;

		let upload = self
			.s3
			.create_multipart_upload()
			.bucket(&self.bucket)
			.key(format!("outbox/{}", self.name))
			.content_type("application/octet-stream")
			.send()
			.await
			.map_err(|err| err.into_service_error())
			.into_diagnostic()?;

		let upload_id = upload
			.upload_id()
			.ok_or_else(|| miette::miette!("upload_id was None"))?;

		let mut tasks = JoinSet::new();
		for (n, chunk) in meta.chunks.keys().enumerate() {
			let s3 = self.s3.clone();
			let bucket = self.bucket.clone();
			let inbox_path = self.inbox_filename(chunk);
			let outbox_path = format!("outbox/{}", self.name);
			let upload_id = upload_id.to_string();
			let part_number = n as i32 + 1;

			tasks.spawn(async move {
				Self::upload_part(
					&s3,
					&bucket,
					&inbox_path,
					&outbox_path,
					&upload_id,
					part_number,
				)
				.await
			});
		}

		let mut parts = Vec::new();
		let mut had_error = false;
		while let Some(result) = tasks.join_next().await {
			match result.into_diagnostic().and_then(|x| x) {
				Ok(part) => parts.push(part),
				Err(e) => {
					tracing::error!("Failed to upload part: {}", e);
					had_error = true;
					break;
				}
			}
		}

		if had_error {
			self.s3
				.abort_multipart_upload()
				.bucket(&self.bucket)
				.key(format!("outbox/{}", self.name))
				.upload_id(upload_id)
				.send()
				.await
				.map_err(|err| err.into_service_error())
				.into_diagnostic()?;
			return Err(miette::miette!("Failed to upload all parts"));
		}

		self.s3
			.complete_multipart_upload()
			.bucket(&self.bucket)
			.key(format!("outbox/{}", self.name))
			.upload_id(upload_id)
			.multipart_upload(
				aws_sdk_s3::types::CompletedMultipartUpload::builder()
					.set_parts(Some(parts))
					.build(),
			)
			.send()
			.await
			.map_err(|err| err.into_service_error())
			.into_diagnostic()?;

		Ok(())
	}

	async fn cleanup(&self) -> Result<()> {
		let mut delete = aws_sdk_s3::types::Delete::builder();
		for prefix in ["inbox", "state"] {
			let list_resp = self
				.s3
				.list_objects_v2()
				.bucket(&self.bucket)
				.prefix(format!("{prefix}/{}", self.name))
				.send()
				.await
				.map_err(|err| err.into_service_error())
				.into_diagnostic()?;

			for obj in list_resp.contents() {
				if let Some(key) = obj.key() {
					delete = delete.objects(
						aws_sdk_s3::types::ObjectIdentifier::builder()
							.key(key)
							.build()
							.into_diagnostic()?,
					);
				}
			}
		}

		self.s3
			.delete_objects()
			.bucket(&self.bucket)
			.delete(delete.build().into_diagnostic()?)
			.send()
			.await
			.map_err(|err| err.into_service_error())
			.into_diagnostic()?;

		Ok(())
	}
}

#[derive(Debug)]
enum LockType {
	Read,
	Write,
}

#[derive(Debug, Clone)]
struct ChunkVersion(String);

impl ChunkVersion {
	async fn fetch_from_s3(
		s3: &aws_sdk_s3::Client,
		bucket: &str,
		key: &str,
	) -> Result<Option<Self>> {
		if let Some(meta) = CHUNK_META_CACHE.lock().await.get(key) {
			return Ok(Some(meta.clone()));
		}

		let resp = s3
			.head_object()
			.bucket(bucket)
			.key(key)
			.send()
			.await
			.map_err(|err| err.into_service_error())
			.map(Some)
			.or_else(|err| {
				if err.meta().code() == Some("404") {
					Ok(None)
				} else {
					Err(err)
				}
			})
			.into_diagnostic()?;

		let Some(resp) = resp else {
			return Ok(None);
		};
		let version_id = resp
			.version_id()
			.ok_or_else(|| miette::miette!("version_id was None"))?
			.to_string();

		let meta = Self(version_id);

		CHUNK_META_CACHE
			.lock()
			.await
			.insert(key.to_string(), meta.clone());

		Ok(Some(meta))
	}
}

#[derive(Debug, Serialize, Deserialize)]
struct ChunkedMetadata {
	pub full_size: u64,
	pub full_sum: String,
	pub chunk_n: u64,
	pub chunk_size: u64,
	pub chunks: BTreeMap<String, String>,
}

impl ChunkedMetadata {
	pub async fn read(s3: &aws_sdk_s3::Client, bucket: &str, prefix: &str) -> Result<Self> {
		let resp = s3
			.get_object()
			.bucket(bucket)
			.key(format!("{prefix}/metadata.json"))
			.send()
			.await
			.map_err(|err| err.into_service_error())
			.into_diagnostic()?;

		let content = resp.body.collect().await.into_diagnostic()?;
		let metadata: Self = serde_json::from_slice(&content.to_vec()).into_diagnostic()?;
		Ok(metadata)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use lambda_runtime::{Context, LambdaEvent};

	#[tokio::test]
	async fn test_event_handler() {
		let event = LambdaEvent::new(S3Event::default(), Context::default());
		let response = function_handler(event).await.unwrap();
		assert_eq!((), response);
	}
}
