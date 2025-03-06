#![deny(rust_2018_idioms)]

use std::env;

use chrono::{DateTime, TimeZone, Utc};

pub use crate::actions::{run, tamanu::find_postgres_bin};
pub use crate::args::get_args as args;

pub(crate) mod actions;
pub(crate) mod args;
#[cfg(feature = "download")]
pub(crate) mod download;

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

/// A wrapper of [`chrono::Utc::now`].
///
/// On debug build, this returns a fixed time if `BESTOOL_MOCK_TIME` is set.
fn now_time<T: TimeZone>(tz: &T) -> DateTime<T> {
	if cfg!(debug_assertions) && env::var("BESTOOL_MOCK_TIME").is_ok() {
		DateTime::from_timestamp_nanos(0)
	} else {
		Utc::now()
	}
	.with_timezone(tz)
}
