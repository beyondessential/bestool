use std::{
	env::var,
	fs::File,
	io::{stderr, Write},
	sync::Mutex,
};

use clap::Subcommand;
use miette::{IntoDiagnostic, Result};
use tokio::fs::metadata;
use tracing::{info, trace, warn, Metadata};
use tracing_subscriber::fmt::MakeWriter;

pub mod completions;
pub mod tamanu;
pub mod upload;

#[derive(Debug, Clone, Subcommand)]
pub enum Action {
	Completions(completions::CompletionsArgs),
	Tamanu(tamanu::TamanuArgs),
	Upload(upload::UploadArgs),
}

pub async fn run() -> Result<()> {
	let ctx = init().await?;
	info!(version=%env!("CARGO_PKG_VERSION"), "starting up");
	trace!(?ctx, "context");

	match ctx.take_top() {
		(Action::Completions(args), ctx) => completions::run(ctx.with_top(args)).await,
		(Action::Tamanu(args), ctx) => tamanu::run(ctx.with_top(args)).await,
		(Action::Upload(args), ctx) => upload::run(ctx.with_top(args)).await,
	}
}

#[derive(Clone, Debug)]
pub(crate) struct Context<A = (), B = ()> {
	pub args_top: A,
	pub args_sub: B,
	pub progress: indicatif::MultiProgress,
}

impl Context {
	pub fn new() -> Self {
		Self {
			args_top: (),
			args_sub: (),
			progress: indicatif::MultiProgress::new(),
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

	pub fn data_bar(&self, len: u64) -> indicatif::ProgressBar {
		self.progress.add(indicatif::ProgressBar::new(len).with_style(
			indicatif::ProgressStyle::default_bar()
				.template("[{bar:20.cyan/blue}] {wide_msg} {bytes}/{total_bytes} [{bytes_per_sec}] ({eta})")
				.expect("data bar template invalid")
		))
	}
}

async fn init() -> Result<Context<Action>> {
	let ctx = Context::new();

	let mut log_on = false;

	if !log_on && var("RUST_LOG").is_ok() {
		match tracing_subscriber::fmt()
			.with_writer(ctx.clone())
			.try_init()
		{
			Ok(_) => {
				warn!(RUST_LOG=%var("RUST_LOG").unwrap(), "logging configured from RUST_LOG");
				log_on = true;
			}
			Err(e) => eprintln!("Failed to initialise logging with RUST_LOG, falling back\n{e}"),
		}
	}

	let args = crate::args::get_args();
	let verbosity = args.verbose.unwrap_or(0);

	if log_on {
		warn!("ignoring logging options from args");
	} else if verbosity > 0 {
		let log_file = if let Some(file) = &args.log_file {
			let is_dir = metadata(&file).await.map_or(false, |info| info.is_dir());
			let path = if is_dir {
				let filename = format!(
					"bestool.{}.log",
					chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ")
				);
				file.join(filename)
			} else {
				file.to_owned()
			};

			Some(File::create(path).into_diagnostic()?)
		} else {
			None
		};

		let mut builder = tracing_subscriber::fmt().with_env_filter(match verbosity {
			0 => unreachable!("checked by if earlier"),
			1 => "warn",
			2 => "info",
			3 => "info,bestool=debug",
			4 => "debug",
			_ => "trace",
		});

		if verbosity > 2 {
			use tracing_subscriber::fmt::format::FmtSpan;
			builder = builder.with_span_events(FmtSpan::NEW | FmtSpan::CLOSE);
		}

		match if let Some(writer) = log_file {
			builder.json().with_writer(Mutex::new(writer)).try_init()
		} else if verbosity > 3 {
			builder.pretty().with_writer(ctx.clone()).try_init()
		} else {
			builder.with_writer(ctx.clone()).try_init()
		} {
			Ok(_) => info!("logging initialised"),
			Err(e) => eprintln!("Failed to initialise logging, continuing with none\n{e}"),
		}
	}

	Ok(ctx.with_top(args.action))
}

#[derive(Debug, Clone)]
pub(crate) struct ProgressLogWriter(indicatif::MultiProgress);

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
