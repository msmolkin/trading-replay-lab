#![forbid(unsafe_code)]

//! Deterministic simulator core. Domain modules use checked integer fixed-point arithmetic.

pub mod instrument;
pub mod numeric;

/// Package identifier used by bootstrap smoke tests and diagnostics.
pub const PACKAGE_NAME: &str = "sim-core";
