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
	//! For example, [`caddy::upgrade::UpgradeArgs`] represents the subcommand:
	//!
	//! ```text
	//! $ bestool caddy upgrade
	//! ```
	//!
	//! and its fields:
	//!
	//! ```
	//! pub struct UpgradeArgs {
	//!     pub version: String,
	//!     pub target: Option<String>,
	//! }
	//! ```
	//!
	//! are transformed into these options:
	//!
	//! ```text
	//! --version
	//! --target
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
