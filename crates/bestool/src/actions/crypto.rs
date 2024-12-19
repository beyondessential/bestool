use std::{
	io::{stderr, IsTerminal as _},
	path::PathBuf,
	str,
};

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressBarIter, ProgressStyle};
use miette::{miette, Context as _, IntoDiagnostic as _, Result};
use regex::Regex;
use tokio::fs::File;

use super::Context;

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

/// Read an age key file from the file specificed by the path
///
/// This ignores any line starting with "#".
#[tracing::instrument(level = "debug")]
async fn read_age_key<T>(path: &PathBuf) -> Result<T>
where
	T: str::FromStr<Err = &'static str>,
{
	let file = tokio::fs::read_to_string(path)
		.await
		.into_diagnostic()
		.wrap_err("reading the key")?;

	let re = Regex::new("#.*").unwrap();
	let identity_string = re.replace_all(&file, "");

	tracing::debug!(?identity_string);

	identity_string
		.trim()
		.parse()
		.map_err(|err: &str| miette!("failed to parse: {err}"))
}
