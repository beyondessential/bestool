//! The Algae simplified encryption command set: support routines and implementation library.
//!
//! # The CLI
//!
//! You can install the CLI tool with:
//!
//! ```console
//! $ cargo install algae-cli
//! ```
//!
//! Algae is a simplified profile of the excellent [age](https://age-encryption.org/v1) format.
//!
//! It implements five functions for the most common operations, and tries to be as obvious and
//! hard-to-misuse as possible, without being prohibitively hard to use, and while retaining
//! forward-compatibility with age (all algae products can be used with age, but not all age
//! products may be used with algae).
//!
//! To start with, generate a keypair with `algae keygen`. This will generate two files:
//! `identity.txt.age`, a passphrase-protected keypair, and `identity.pub`, the public key in plain.
//!
//! To encrypt a file, use `algae encrypt -k identity.pub filename`. As this uses the public key, it
//! doesn't require a passphrase. The encrypted file is written to `filename.age`. To decrypt it,
//! use `algae decrypt -k identity.txt.age filename.age`. As this uses the secret key, it will
//! prompt for its passphrase. The decoded file is written back to `filename` (i.e. without the
//! `.age` suffix).
//!
//! To obtain a plaintext `identity.txt` (i.e. to remove the passphrase), use
//! `algae reveal identity.txt.age`. To add a new passphrase on a plaintext identity, use
//! `algae protect identity.txt`. These commands are not special to identity files: you can
//! `protect` (encrypt) and `reveal` (decrypt) arbitrary files with a passphrase.
//!
//! # The profile
//!
//! - Keypair-based commands use [X25519](age::x25519).
//! - Passphrase-based commands use [scrypt](age::scrypt).
//! - Plugins are not supported.
//! - Multiple recipients are not supported (age-produced multi-recipient files _may_ be decrypted).
//! - Identity files with multiple identities are not supported (they _might_ work, but behaviour is unspecified).
//! - Passphrase entry is done with `pinentry` when available, and falls back to a terminal prompt.
//!
//! # The library
//!
//! This crate is a little atypical in that it deliberately exposes the CLI support structures as
//! its library, for the purpose of embedding part or parcel of the Algae command set or conventions
//! into other tools.
//!
//! For example, you can insert Algae's `encrypt` and `decrypt` in a [tokio] + [clap] program:
//!
//! ```no_run
//! use clap::Parser;
//! use miette::Result;
//! use algae_cli::cli::*;
//!
//! /// Your CLI tool
//! #[derive(Parser)]
//! enum Command {
//!     Encrypt(encrypt::EncryptArgs),
//!     Decrypt(decrypt::DecryptArgs),
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let command = Command::parse();
//!     match command {
//!         Command::Encrypt(args) => encrypt::run(args).await,
//!         Command::Decrypt(args) => decrypt::run(args).await,
//!     }
//! }
//! ```
//!
//! Or you can prompt for a passphrase with the same flags and logic as algae with:
//!
//! ```no_run
//! use age::secrecy::ExposeSecret;
//! use algae_cli::passphrases::PassphraseArgs;
//! use clap::Parser;
//! use miette::Result;
//!
//! /// Your CLI tool
//! #[derive(Parser)]
//! struct Args {
//!     your_args: bool,
//!
//!     #[command(flatten)]
//!     pass: PassphraseArgs,
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let args = Args::parse();
//!     let key = args.pass.require_phrase().await?;
//!     dbg!(key.expose_secret());
//!     Ok(())
//! }
//! ```
//!
//! Or you can add optional file encryption to a tool:
//!
//! ```no_run
//! use std::path::PathBuf;
//!
//! use algae_cli::{keys::KeyArgs, streams::encrypt_stream};
//! use clap::Parser;
//! use miette::{IntoDiagnostic, Result, WrapErr};
//! use tokio::fs::File;
//! use tokio_util::compat::TokioAsyncWriteCompatExt;
//!
//! /// Your CLI tool
//! ///
//! /// If `--key` or `--key-path` is provided, the file will be encrypted.
//! #[derive(Parser)]
//! struct Args {
//!     output_path: PathBuf,
//!
//!     #[command(flatten)]
//!     key: KeyArgs,
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let args = Args::parse();
//!
//!     // if a key is provided, validate it early
//!     let key = args.key.get_public_key().await?;
//!
//!     let mut input = generate_file_data_somehow().await;
//!     let mut output = File::create_new(args.output_path)
//!         .await
//!         .into_diagnostic()
//!         .wrap_err("opening the output file")?;
//!
//!     if let Some(key) = key {
//!         encrypt_stream(input, output.compat_write(), key).await?;
//!     } else {
//!         tokio::io::copy(&mut input, &mut output)
//!             .await
//!             .into_diagnostic()
//!             .wrap_err("copying data to file")?;
//!     }
//!
//!     Ok(())
//! }
//!
//! # async fn generate_file_data_somehow() -> &'static [u8] { &[] }
//! ```
//!
//! # The name
//!
//! _age_ is pronounced ah-gay. While [age doesn't have an inherent meaning](https://github.com/FiloSottile/age/discussions/329),
//! the Italian-adjacent Friulian language (spoken around Venice) word _aghe_, pronounced the same, means water.
//!
//! Algae (pronounced al-gay or al-ghee) is a lightweight (a)ge. Algae are also fond of water.

#![deny(rust_2018_idioms)]
#![deny(unsafe_code)]
#![deny(missing_docs)]

/// Clap argument parsers and implementations for the algae CLI functions.
pub mod cli;

/// Support for encrypting and decrypting files.
pub mod files;

/// Support for obtaining public and secret keys.
pub mod keys;

/// Support for obtaining passphrases.
pub mod passphrases;

/// Support for encrypting and decrypting bytestreams.
pub mod streams;
