//! Deterministic execution models by market-data fidelity tier.

// F0 intentionally keeps its order transition in one auditable function; its sole `expect`
// asserts an internal precomputation invariant established earlier in that same transition.
#[allow(clippy::missing_panics_doc, clippy::too_many_lines)]
pub mod f0;
pub mod f1;
// Keep optional-side presence and crossed-book validation as two explicit audit steps.
#[allow(clippy::collapsible_if)]
pub mod f2;
