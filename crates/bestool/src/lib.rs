#![deny(rust_2018_idioms)]

pub use crate::actions::run;
pub use crate::args::get_args as args;

pub(crate) mod actions;
pub(crate) mod args;
#[cfg(feature = "aws")]
pub(crate) mod aws;
pub mod file_chunker;

pub(crate) mod postgres_to_value;

#[allow(dead_code)] // some subcommands don't use it, but it's easier to have it everywhere
pub(crate) const APP_NAME: &str = concat!(env!("CARGO_PKG_NAME"), "-", env!("CARGO_PKG_VERSION"));
