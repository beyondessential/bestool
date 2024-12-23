use std::io::{stderr, IsTerminal as _};

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressBarIter, ProgressStyle};
use miette::Result;
use tokio::io::AsyncRead;

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

/// Wraps a [`tokio::io::AsyncRead`] with an [`indicatif::ProgressBar`].
///
/// The progress bar outputs to stderr iff that's terminal, and nothing is displayed otherwise.
pub(crate) fn with_progress_bar<R: AsyncRead + Unpin>(
	expected_length: u64,
	reader: R,
) -> ProgressBarIter<R> {
	if stderr().is_terminal() {
		let style = ProgressStyle::default_bar()
			.template("[{bar:.green/blue}] {wide_msg} {binary_bytes}/{binary_total_bytes} ({eta})")
			.expect("BUG: progress bar template invalid");
		ProgressBar::new(expected_length).with_style(style)
	} else {
		ProgressBar::hidden()
	}
	.wrap_async_read(reader)
}
