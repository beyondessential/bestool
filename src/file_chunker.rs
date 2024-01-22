use std::{
	fs::Metadata,
	future::Future,
	io::SeekFrom,
	num::{NonZeroU64, NonZeroU8},
	path::Path,
};

use bytes::{Bytes, BytesMut};
use miette::{IntoDiagnostic, Result};
use tokio::{
	fs::File,
	io::{AsyncReadExt, AsyncSeekExt},
};
use tracing::{debug, instrument, trace};

/// Absolute minimum chunk size: 100 kB
///
/// Also see [`crate::aws::MINIMUM_MULTIPART_PART_SIZE`].
// SAFETY: hardcoded
pub const MIN_CHUNK_SIZE: NonZeroU64 = unsafe { NonZeroU64::new_unchecked(100 * 1024) };

/// Default initial chunk size: 10 MB
// SAFETY: hardcoded
pub const DEFAULT_CHUNK_SIZE: NonZeroU64 = unsafe { NonZeroU64::new_unchecked(10 * 1024 * 1024) };

/// Default downsizing factor: 5%.
pub const DEFAULT_DOWNSIZE_FACTOR: f64 = 0.95;

/// Default number of tries to attempt per chunk: 10.
// SAFETY: hardcoded
pub const DEFAULT_TRIES_PER_CHUNK: NonZeroU8 = unsafe { NonZeroU8::new_unchecked(10) };

/// Provides an interface to read a file in adaptively-sized chunks.
#[derive(Debug)]
pub struct FileChunker {
	file: File,
	metadata: Metadata,

	/// The chunk size that will be used for the next chunk of bytes read from the file.
	///
	/// This is enforced to be non-zero, but it should also be [`MIN_CHUNK_SIZE`] or above. It isn't
	/// unsafe or unsound for it to be less than [`MIN_CHUNK_SIZE`], 'merely' less efficient.
	pub chunk_size: NonZeroU64,

	/// The minimum chunk size for this [`FileChunker`]. This will be the floor beyond which this
	/// chunker cannot go below. By default this is [`MIN_CHUNK_SIZE`], but it can be useful to set
	/// this to avoid chunking too much while still having adaptive chunking.
	///
	/// This is enforced to be non-zero, but it should also be [`MIN_CHUNK_SIZE`] or above. It isn't
	/// unsafe or unsound for it to be less than [`MIN_CHUNK_SIZE`], 'merely' less efficient.
	pub min_chunk_size: NonZeroU64,

	/// The factor by which to multiply the chunk size when handling a chunk fails. This is how the
	/// adaptive chunk size is controlled, and is used by [`with_next_chunk`](Self::with_next_chunk)
	/// only.
	///
	/// The value is clamped to [0, 1] on usage. A value of 1 will effectively disable adaptive size
	/// adjustment. A value too close to 0 will downsize too rapidly to be useful. It's recommended
	/// to stick to the range [0.80, 0.99] for most uses.
	///
	/// See [`DEFAULT_DOWNSIZE_FACTOR`] for the initial value.
	pub downsize_factor: f64,

	/// How many times a chunk will be retried before erroring out. This is used by
	/// [`with_next_chunk`](Self::with_next_chunk) only.
	///
	/// See [`DEFAULT_TRIES_PER_CHUNK`] for the initial value.
	pub tries_per_chunk: NonZeroU8,

	chunks: u64,
	previous_chunk_offset: Option<u64>,
}

impl FileChunker {
	/// Create a new chunker with the [default initial chunk size](DEFAULT_CHUNK_SIZE).
	///
	/// See the struct fields documentation for
	/// [`downsize_factor`](Self#structfield.downsize_factor) and
	/// [`tries_per_chunk`](Self#structfield.tries_per_chunk).
	#[instrument(level = "debug")]
	pub async fn new(file: &Path) -> Result<Self> {
		Self::with_chunk_size(file, DEFAULT_CHUNK_SIZE.get()).await
	}

	/// Create a new chunker with a custom initial chunk size.
	///
	/// If the size given is smaller than [`min_chunk_size`](Self#structfield.min_chunk_size), that
	/// will be used instead.
	#[instrument(level = "debug")]
	pub async fn with_chunk_size(file: &Path, chunk_size: u64) -> Result<Self> {
		let file = File::open(file).await.into_diagnostic()?;
		let metadata = file.metadata().await.into_diagnostic()?;
		Ok(Self {
			file,
			metadata,
			// SAFETY: MIN_CHUNK_SIZE is non-zero, so this value is always non-zero
			chunk_size: unsafe { NonZeroU64::new_unchecked(chunk_size.max(MIN_CHUNK_SIZE.get())) },
			min_chunk_size: MIN_CHUNK_SIZE,
			downsize_factor: DEFAULT_DOWNSIZE_FACTOR,
			tries_per_chunk: DEFAULT_TRIES_PER_CHUNK,
			chunks: 0,
			previous_chunk_offset: None,
		})
	}

	/// The length of the file in bytes.
	///
	/// This is read from the file metadata when this struct is created.
	#[inline]
	#[allow(clippy::len_without_is_empty)]
	pub fn len(&self) -> u64 {
		self.metadata.len()
	}

	/// The number of processed chunks so far.
	#[inline]
	pub fn chunks(&self) -> u64 {
		self.chunks
	}

	/// Reset the reader to the start of the chunk that was just read, and lower the chunk size.
	///
	/// Use this when an upload fails, to retry with a smaller chunk size.
	///
	/// Chunk size will not be lowered below [`min_chunk_size`](Self#structfield.min_chunk_size).
	#[instrument(skip(self), level = "debug")]
	pub async fn redo_chunk(&mut self, factor: f64) -> Result<()> {
		// SAFETY: min_chunk_size is non-zero, so this value is always non-zero
		self.chunk_size = unsafe {
			NonZeroU64::new_unchecked(
				((self.chunk_size.get() as f64 * factor) as u64).max(self.min_chunk_size.get()),
			)
		};

		if let Some(offset) = self.previous_chunk_offset.take() {
			self.file
				.seek(SeekFrom::Start(offset))
				.await
				.into_diagnostic()?;
		}

		Ok(())
	}

	/// Read the next chunk from the file.
	///
	/// Returns `Ok(None)` if the end of the file has been reached.
	#[instrument(skip(self), level = "debug")]
	pub async fn next(&mut self) -> Result<Option<Bytes>> {
		let start = self.file.stream_position().await.into_diagnostic()?;
		let remaining = self.len().saturating_sub(start);

		if remaining == 0 {
			return Ok(None);
		}

		let size = self.chunk_size.get().min(remaining);
		trace!(start, remaining, size, "reading a chunk of file");

		let mut chunk = BytesMut::with_capacity(size as _);
		let mut bytes = self.file.read_buf(&mut chunk).await.into_diagnostic()?;
		trace!(bytes, "read some bytes from the file");
		while bytes > 0 && chunk.len() < size as _ {
			let more_bytes = self.file.read_buf(&mut chunk).await.into_diagnostic()?;
			trace!(
				bytes = more_bytes,
				total = bytes,
				"read some more bytes from the file"
			);
			bytes += more_bytes;
		}
		chunk.truncate(bytes);
		debug!(bytes, "read a chunk of bytes from the file");

		if bytes > 0 {
			self.chunks += 1;
			self.previous_chunk_offset = Some(start);
			Ok(Some(chunk.into()))
		} else {
			Ok(None)
		}
	}

	/// Read the next chunk from the file, and call the given handler with it.
	///
	/// If the handler returns an error, the chunk will be re-read with a smaller size as defined by
	/// [`downsize_factor`](Self#structfield.downsize_factor), and the handler will be called again.
	/// This will be repeated until the handler returns `Ok(())`, or until the number of
	/// [`tries_per_chunk`](Self#structfield.tries_per_chunk) has been reached.
	///
	/// Returns the error from the last attempt if it doesn't succeed even after all its tries, and
	/// a the number of bytes in the last chunk read otherwise, or `None` if the end of the file has
	/// been reached. Note that `Ok(Some(_))` will be returned for the last chunk of the file, and
	/// one more call (which will not call the handler) is required to get `Ok(None)`.
	#[instrument(skip(self, handler), level = "debug")]
	pub async fn with_next_chunk<H, F, T>(&mut self, handler: &H) -> Result<Option<(u64, T)>>
	where
		for<'a> H: Fn(Bytes) -> F + 'a,
		F: Future<Output = Result<T>>,
	{
		let mut last_error = None;
		for _ in 0..self.tries_per_chunk.get() {
			let Some(chunk) = self.next().await? else {
				return Ok(None);
			};

			let real_size = chunk.len();
			match handler(chunk).await {
				Err(err) => {
					last_error = Some(err);
					self.redo_chunk(self.downsize_factor.clamp(0_f64, 1_f64))
						.await?;
				}
				Ok(t) => return Ok(Some((real_size as _, t))),
			}
		}

		// UNWRAP: last_error is Some if we got here, because tries is non-zero
		Err(last_error.unwrap())
	}
}
