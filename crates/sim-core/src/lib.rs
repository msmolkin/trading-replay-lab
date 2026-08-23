#![forbid(unsafe_code)]

//! Deterministic simulator core. Domain modules use checked integer fixed-point arithmetic.

pub mod hash;
pub mod instrument;
pub mod kernel;
pub mod ledger;
pub mod numeric;
pub mod orders;
pub mod positions;

/// Package identifier used by bootstrap smoke tests and diagnostics.
pub const PACKAGE_NAME: &str = "sim-core";
