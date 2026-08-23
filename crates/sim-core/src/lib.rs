#![forbid(unsafe_code)]

//! Deterministic simulator core. Domain modules use checked integer fixed-point arithmetic.

pub mod hash;
pub mod instrument;
pub mod kernel;
pub mod numeric;
pub mod orders;

/// Package identifier used by bootstrap smoke tests and diagnostics.
pub const PACKAGE_NAME: &str = "sim-core";
