#![deny(rust_2018_idioms)]

pub use crate::actions::run;

pub(crate) mod actions;
pub(crate) mod args;
#[cfg(feature = "aws")]
pub(crate) mod aws;
pub mod file_chunker;

pub(crate) const APP_NAME: &str = concat!(env!("CARGO_PKG_NAME"), "-", env!("CARGO_PKG_VERSION"));
