use std::env;

pub use crate::actions::run_with_update_check as run;
pub use crate::args::get_args as args;

pub(crate) mod actions;
pub(crate) mod args;
#[cfg(all(
	test,
	feature = "canopy-register",
	feature = "tamanu-artifacts",
	feature = "tamanu-psql"
))]
mod canopy_contract;
#[cfg(feature = "download")]
pub(crate) mod download;
pub mod find_postgres;
pub(crate) mod http;

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

/// A wrapper of [`jiff::Timestamp::now`].
///
/// On debug build, this returns the unix epoch if `BESTOOL_MOCK_TIME` is set.
fn now_time() -> jiff::Timestamp {
	if cfg!(debug_assertions) && env::var("BESTOOL_MOCK_TIME").is_ok() {
		jiff::Timestamp::UNIX_EPOCH
	} else {
		jiff::Timestamp::now()
	}
}
