#![forbid(unsafe_code)]

//! Offline verification primitives for portable replay proof bundles.
//!
//! M3-08 exports a proof package containing the public result bundle plus the canonical kernel
//! inputs required to reproduce the simulator event-chain commitment. This crate verifies those
//! inputs without network access and keeps exact financial values as integers throughout.

mod bundle;
mod json;
mod ledger;

pub use ledger::LedgerInspection;

use bundle::{
    ProofBundle, manifest_commitment, result_commitment, state_hash_commitment, verify_state_hashes,
};
use ledger::verify_ledger;
use sim_core::hash::{ZERO_HASH, hash_hex};
use sim_core::kernel::Kernel;

/// Machine-readable verification outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationReport {
    /// Whether every proof commitment and invariant matched.
    pub valid: bool,
    /// Exact failure classification and location when invalid.
    pub failure: Option<VerificationFailure>,
    /// Reproduced kernel chain head when input replay reached completion.
    pub final_event_hash: Option<String>,
    /// Recomputed final result commitment when all prerequisite sections were valid.
    pub computed_result_hash: Option<String>,
    /// Number of canonical kernel inputs successfully reproduced.
    pub inputs_verified: usize,
    /// Number of balanced ledger transactions verified.
    pub ledger_transactions_verified: usize,
}

impl VerificationReport {
    fn fail(
        failure: VerificationFailure,
        inputs_verified: usize,
        ledger_transactions_verified: usize,
        final_event_hash: Option<String>,
    ) -> Self {
        Self {
            valid: false,
            failure: Some(failure),
            final_event_hash,
            computed_result_hash: None,
            inputs_verified,
            ledger_transactions_verified,
        }
    }

    /// Renders stable JSON without depending on a serialization crate.
    #[must_use]
    pub fn to_json(&self) -> String {
        let failure = self.failure.as_ref().map_or_else(
            || "null".to_owned(),
            |failure| {
                format!(
                    "{{\"code\":\"{}\",\"index\":{},\"detail\":\"{}\"}}",
                    failure.code.code(),
                    failure
                        .index
                        .map_or_else(|| "null".to_owned(), |value| value.to_string()),
                    json_escape(&failure.detail)
                )
            },
        );
        format!(
            "{{\"valid\":{},\"failure\":{},\"final_event_hash\":{},\"computed_result_hash\":{},\"inputs_verified\":{},\"ledger_transactions_verified\":{}}}",
            self.valid,
            failure,
            json_option_string(self.final_event_hash.as_deref()),
            json_option_string(self.computed_result_hash.as_deref()),
            self.inputs_verified,
            self.ledger_transactions_verified,
        )
    }
}

/// Precise proof verification failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationFailure {
    /// Stable failure family.
    pub code: VerificationFailureCode,
    /// Zero-based failing section entry when applicable.
    pub index: Option<usize>,
    /// Human-readable diagnostic with no secret material.
    pub detail: String,
}

impl VerificationFailure {
    fn new(code: VerificationFailureCode, index: Option<usize>, detail: impl Into<String>) -> Self {
        Self {
            code,
            index,
            detail: detail.into(),
        }
    }
}

/// Stable machine failure classifications.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerificationFailureCode {
    Format,
    ManifestHash,
    ManifestCommitment,
    InputSequence,
    EventMismatch,
    StateHash,
    StateCommitment,
    LedgerFormat,
    LedgerImbalance,
    LedgerCommitment,
    ResultCommitment,
}

impl VerificationFailureCode {
    /// Stable wire code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Format => "FORMAT",
            Self::ManifestHash => "MANIFEST_HASH",
            Self::ManifestCommitment => "MANIFEST_COMMITMENT",
            Self::InputSequence => "INPUT_SEQUENCE",
            Self::EventMismatch => "EVENT_MISMATCH",
            Self::StateHash => "STATE_HASH",
            Self::StateCommitment => "STATE_COMMITMENT",
            Self::LedgerFormat => "LEDGER_FORMAT",
            Self::LedgerImbalance => "LEDGER_IMBALANCE",
            Self::LedgerCommitment => "LEDGER_COMMITMENT",
            Self::ResultCommitment => "RESULT_COMMITMENT",
        }
    }
}

/// Verifies a complete proof bundle from UTF-8 JSON bytes.
#[must_use]
pub fn verify_bytes(bytes: &[u8]) -> VerificationReport {
    let root = match json::parse(bytes) {
        Ok(value) => value,
        Err(error) => {
            return VerificationReport::fail(
                VerificationFailure::new(
                    VerificationFailureCode::Format,
                    None,
                    format!("JSON {:?} at byte {}", error.kind, error.offset),
                ),
                0,
                0,
                None,
            );
        }
    };
    let bundle = match ProofBundle::from_json(&root) {
        Ok(bundle) => bundle,
        Err(failure) => return VerificationReport::fail(failure, 0, 0, None),
    };
    verify_bundle(&bundle)
}

/// Inspects and validates only the balanced ledger section of a proof bundle.
pub fn inspect_ledger_bytes(bytes: &[u8]) -> Result<LedgerInspection, VerificationFailure> {
    let root = json::parse(bytes).map_err(|error| {
        VerificationFailure::new(
            VerificationFailureCode::Format,
            None,
            format!("JSON {:?} at byte {}", error.kind, error.offset),
        )
    })?;
    let bundle = ProofBundle::from_json(&root)?;
    verify_ledger(&bundle.ledger_transactions, &bundle.ledger_hash)
}

fn verify_bundle(bundle: &ProofBundle) -> VerificationReport {
    let manifest_hash = manifest_commitment(&bundle.manifest_hashes);
    if manifest_hash != bundle.manifest_set_hash {
        return VerificationReport::fail(
            VerificationFailure::new(
                VerificationFailureCode::ManifestCommitment,
                None,
                "manifest set commitment does not match manifest_hashes",
            ),
            0,
            0,
            None,
        );
    }
    if bundle.inputs.len() != bundle.kernel_events.len() {
        return VerificationReport::fail(
            VerificationFailure::new(
                VerificationFailureCode::EventMismatch,
                None,
                "inputs and kernel_events must have equal length",
            ),
            0,
            0,
            None,
        );
    }

    let mut kernel = Kernel::new();
    let mut verified = 0_usize;
    let mut final_hash = ZERO_HASH;
    for (index, (input, declared)) in bundle.inputs.iter().zip(&bundle.kernel_events).enumerate() {
        if input.session_id != bundle.session_id {
            return verification_failure(
                VerificationFailureCode::InputSequence,
                Some(index),
                "input session_id differs from proof session_id",
                verified,
                final_hash,
            );
        }
        let actual = match kernel.apply(input) {
            Ok(event) => event,
            Err(error) => {
                return verification_failure(
                    VerificationFailureCode::InputSequence,
                    Some(index),
                    error.to_string(),
                    verified,
                    final_hash,
                );
            }
        };
        if !declared.matches(&actual) {
            return verification_failure(
                VerificationFailureCode::EventMismatch,
                Some(index),
                format!(
                    "declared kernel event {} differs from reproduced event",
                    declared.event_seq
                ),
                verified,
                final_hash,
            );
        }
        final_hash = actual.current_event_hash;
        verified += 1;
    }

    if let Err(failure) = verify_state_hashes(&bundle.state_hashes, &bundle.state_hashes_hash) {
        return VerificationReport::fail(failure, verified, 0, Some(hash_hex(&final_hash)));
    }
    let state_commitment = state_hash_commitment(&bundle.state_hashes);
    let ledger = match verify_ledger(&bundle.ledger_transactions, &bundle.ledger_hash) {
        Ok(ledger) => ledger,
        Err(failure) => {
            return VerificationReport::fail(failure, verified, 0, Some(hash_hex(&final_hash)));
        }
    };

    let computed_result = result_commitment(
        &bundle.session_id,
        &manifest_hash,
        &final_hash,
        &state_commitment,
        &bundle.ledger_hash,
        bundle.metrics,
    );
    if computed_result != bundle.result_hash {
        return VerificationReport::fail(
            VerificationFailure::new(
                VerificationFailureCode::ResultCommitment,
                None,
                "result_hash does not match reproduced proof commitments and metrics",
            ),
            verified,
            ledger.transactions,
            Some(hash_hex(&final_hash)),
        );
    }

    VerificationReport {
        valid: true,
        failure: None,
        final_event_hash: Some(hash_hex(&final_hash)),
        computed_result_hash: Some(hash_hex(&computed_result)),
        inputs_verified: verified,
        ledger_transactions_verified: ledger.transactions,
    }
}

fn verification_failure(
    code: VerificationFailureCode,
    index: Option<usize>,
    detail: impl Into<String>,
    inputs_verified: usize,
    final_hash: [u8; 32],
) -> VerificationReport {
    VerificationReport::fail(
        VerificationFailure::new(code, index, detail),
        inputs_verified,
        0,
        Some(hash_hex(&final_hash)),
    )
}

fn json_option_string(value: Option<&str>) -> String {
    value.map_or_else(
        || "null".to_owned(),
        |value| format!("\"{}\"", json_escape(value)),
    )
}

pub(crate) fn json_escape(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(&mut output, "\\u{:04x}", u32::from(character));
            }
            character => output.push(character),
        }
    }
    output
}

#[cfg(test)]
mod tests;
