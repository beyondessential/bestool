use std::{io::{stderr, Write}, sync::{Arc, RwLock}, mem::swap, ops::DerefMut};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tracing::Metadata;
use tracing_subscriber::fmt::MakeWriter;

use super::upload::UploadId;

#[derive(Clone, Debug)]
pub struct Context<A = (), B = ()> {
	pub args_top: A,
	pub args_sub: B,
	pub progress: MultiProgress,
	cleanups: Arc<RwLock<boxcar::Vec<Cleanup>>>,
}

impl Context {
	pub fn new() -> Self {
		Self {
			args_top: (),
			args_sub: (),
			progress: MultiProgress::new(),
			cleanups: Arc::new(RwLock::new(boxcar::Vec::new())),
		}
	}
}

impl<A, B> Context<A, B> {
	pub fn with_top<C>(self, args_top: C) -> Context<C, B> {
		Context::<C, B> {
			args_top,
			args_sub: self.args_sub,
			progress: self.progress,
			cleanups: self.cleanups,
		}
	}

	pub fn with_sub<C>(self, args_sub: C) -> Context<A, C> {
		Context::<A, C> {
			args_top: self.args_top,
			args_sub,
			progress: self.progress,
			cleanups: self.cleanups,
		}
	}

	pub fn take_top(self) -> (A, Context<(), B>) {
		(
			self.args_top,
			Context::<(), B> {
				args_top: (),
				args_sub: self.args_sub,
				progress: self.progress,
				cleanups: self.cleanups,
			},
		)
	}

	pub fn bar(&self, len: u64) -> ProgressBar {
		self.progress.add(ProgressBar::new(len).with_style(
			ProgressStyle::default_bar()
				.template("[{bar:.green/blue}] {wide_msg} {human_pos}/{human_len} ({eta})")
				.expect("bar template invalid")
		))
	}

	pub fn data_bar(&self, len: u64) -> ProgressBar {
		self.progress.add(ProgressBar::new(len).with_style(
			ProgressStyle::default_bar()
				.template("[{bar:.cyan/blue}] {wide_msg} {bytes}/{total_bytes} [{bytes_per_sec}] ({eta})")
				.expect("data bar template invalid")
		))
	}

	pub fn erased(&self) -> Context<(), ()> {
		Context::<(), ()> {
			args_top: (),
			args_sub: (),
			progress: self.progress.clone(),
			cleanups: self.cleanups.clone(),
		}
	}

	// TODO: clean up on ctrl-c
	pub fn add_cleanup(&self, cleanup: Cleanup) {
		self.cleanups.read().unwrap().push(cleanup);
	}

	pub fn process_cleanups(&self) -> Vec<Cleanup> {
		let mut guard = self.cleanups.write().unwrap();
		let locked = guard.deref_mut();
		let mut retrieved = boxcar::Vec::new();
		swap(locked, &mut retrieved);
		retrieved.into_iter().collect()
	}
}

#[derive(Debug, Clone)]
pub struct ProgressLogWriter(MultiProgress);

impl Write for ProgressLogWriter {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		self.0.suspend(|| stderr().write(buf))
	}

	fn flush(&mut self) -> std::io::Result<()> {
		self.0.suspend(|| stderr().flush())
	}
}

impl<'w, A, B> MakeWriter<'w> for Context<A, B> {
	type Writer = ProgressLogWriter;

	fn make_writer(&'w self) -> Self::Writer {
		ProgressLogWriter(self.progress.clone())
	}

	fn make_writer_for(&'w self, _meta: &Metadata<'_>) -> Self::Writer {
		ProgressLogWriter(self.progress.clone())
	}
}

#[derive(Debug, Clone)]
pub enum Cleanup {
	MultiPartUpload(UploadId),
}
