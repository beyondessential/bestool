//! Backwards-compatible alias for the top-level `bestool alertd` command.
//!
//! The daemon moved out of the `tamanu` namespace (it now also serves non-Tamanu
//! hosts), but `bestool tamanu alert` / `bestool tamanu alertd` keep working —
//! both delegate to [`crate::actions::alertd`].
pub use crate::actions::alertd::{AlertdArgs as AlertArgs, run};
