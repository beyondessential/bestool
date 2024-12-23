use std::io::{stderr, IsTerminal as _};

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressBarIter, ProgressStyle};
use miette::{IntoDiagnostic, Result};
use tokio::fs::File;

use super::Context;

pub mod key;

/// Cryptographic operations.
#[derive(Debug, Clone, Parser)]
pub struct CryptoArgs {
	/// Crypto subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[Context<CryptoArgs> => {|ctx: Context<CryptoArgs>| -> Result<(Action, Context<CryptoArgs>)> {
		Ok((ctx.args_top.action.clone(), ctx.with_sub(())))
	}}](with_sub)

	decrypt => Decrypt(DecryptArgs),
	encrypt => Encrypt(EncryptArgs),
	hash => Hash(HashArgs),
	keygen => Keygen(KeygenArgs)
}

/// Wraps a [`tokio::fs::File`] with a [`indicatif::ProgressBar`].
///
/// The progress bar outputs to stderr. This does nothing if stderr is not terminal.
async fn wrap_async_read_with_progress_bar(read: File) -> Result<ProgressBarIter<File>> {
	let progress_bar = if stderr().is_terminal() {
		let style = ProgressStyle::default_bar()
			.template("[{bar:.green/blue}] {wide_msg} {binary_bytes}/{binary_total_bytes} ({eta})")
			.expect("bar template invalid");
		ProgressBar::new(read.metadata().await.into_diagnostic()?.len()).with_style(style)
	} else {
		ProgressBar::hidden()
	};

	Ok(progress_bar.wrap_async_read(read))
}
