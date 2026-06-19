//! `bestool tamanu tags` — deprecated alias for `bestool canopy tags`.
//!
//! The implementation moved to [`crate::actions::canopy::tags`]; this module
//! re-exports it so the old invocation keeps working.

pub use crate::actions::canopy::tags::{TagsArgs, run};
