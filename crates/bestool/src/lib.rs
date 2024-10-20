#![deny(rust_2018_idioms)]

pub use crate::actions::run;
pub use crate::args::get_args as args;

pub(crate) mod actions;
pub(crate) mod args;
#[cfg(feature = "aws")]
pub(crate) mod aws;
pub mod file_chunker;

#[cfg(feature = "tamanu-alerts")]
pub(crate) mod postgres_to_value;

#[allow(dead_code)] // some subcommands don't use it, but it's easier to have it everywhere
pub(crate) const APP_NAME: &str = concat!(env!("CARGO_PKG_NAME"), "-", env!("CARGO_PKG_VERSION"));

#[cfg(doc)]
pub mod __help {
	//! Documentation-only module containing the help pages for the CLI tool.
	//!
	//! The [`Args`] struct contains the top level options. The [`Action`] enum contains the top
	//! level subcommands. Beyond that, `*Args` structs contain options for that level, and
	//! `*Action` enums contain subcommands below that level. In structs, field names are
	//! generally transformed to options using by being kebab-cased.
	//!
	//! For example, [`caddy::configure_tamanu::ConfigureTamanuArgs`] represents the subcommand:
	//!
	//! ```text
	//! $ bestool caddy configure-tamanu
	//! ```
	//!
	//! and its fields:
	//!
	//! ```
	//! # use std::{num::NonZeroU16, path::PathBuf};
	//! pub struct ConfigureTamanuArgs {
	//!     pub path: PathBuf,
	//!     pub print: bool,
	//!     pub domain: String,
	//!     pub api_port: NonZeroU16,
	//!     pub api_version: String,
	//!     pub web_version: String,
	//!     pub email: Option<String>,
	//!     pub zerossl_api_key: Option<String>,
	//! }
	//! ```
	//!
	//! are transformed into these options:
	//!
	//! ```text
	//! --path
	//! --print
	//! --domain
	//! --api-port
	//! --api-version
	//! --web-version
	//! --email
	//! --zerossl-api-key
	//! ```
	//!
	//! Sometimes more information is contained in the `#[clap()]` attributes like defaults and
	//! positionals, and these can be seen by clicking the `source` link at the top right.

	pub use crate::actions::*;
	pub use crate::args::Args;
}
