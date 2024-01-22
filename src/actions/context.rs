use std::io::{stderr, Write};

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use tracing::Metadata;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Debug)]
pub struct Context<A = (), B = ()> {
	pub args_top: A,
	pub args_sub: B,
	pub progress: MultiProgress,
}

impl Context {
	pub fn new() -> Self {
		Self {
			args_top: (),
			args_sub: (),
			progress: MultiProgress::new(),
		}
	}
}

impl<A, B> Context<A, B> {
	pub fn with_top<C>(self, args_top: C) -> Context<C, B> {
		Context::<C, B> {
			args_top,
			args_sub: self.args_sub,
			progress: self.progress,
		}
	}

	pub fn with_sub<C>(self, args_sub: C) -> Context<A, C> {
		Context::<A, C> {
			args_top: self.args_top,
			args_sub,
			progress: self.progress,
		}
	}

	pub fn take_top(self) -> (A, Context<(), B>) {
		(
			self.args_top,
			Context::<(), B> {
				args_top: (),
				args_sub: self.args_sub,
				progress: self.progress,
			},
		)
	}

	pub fn data_bar(&self, len: u64) -> ProgressBar {
		self.progress.add(ProgressBar::new(len).with_style(
			ProgressStyle::default_bar()
				.template("[{bar:20.cyan/blue}] {wide_msg} {bytes}/{total_bytes} [{bytes_per_sec}] ({eta})")
				.expect("data bar template invalid")
		))
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
