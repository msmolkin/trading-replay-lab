use std::fmt::Write as _;

use sim_core::hash::{hash_hex, sha256};
use sim_core::kernel::{InputEnvelope, Kernel};

use super::*;
use crate::bundle::{
    ResultMetrics, StateHash, manifest_commitment, result_commitment, state_hash_commitment,
    zero_hash,
};
use crate::ledger::{LedgerPosting, LedgerTransaction, ledger_commitment};

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn valid_bundle_json() -> String {
    let input = InputEnvelope {
        session_id: "session-1".into(),
        input_seq: 0,
        expected_state_version: 0,
        logical_ts_ns: 1,
        kind: "COMMAND_TEST".into(),
        payload: b"abc".to_vec(),
    };
    let mut kernel = Kernel::new();
    let event = kernel.apply(&input).unwrap();
    assert_eq!(event.prior_event_hash, zero_hash());

    let manifests = vec![sha256(b"manifest-a")];
    let manifest_set_hash = manifest_commitment(&manifests);
    let states = vec![StateHash {
        event_seq: 0,
        hash: event.current_event_hash,
    }];
    let state_hashes_hash = state_hash_commitment(&states);
    let ledger = vec![LedgerTransaction {
        event_seq: 0,
        transaction_id: "tx-1".into(),
        postings: vec![
            LedgerPosting {
                account: "CASH".into(),
                amount_minor: 10,
                currency: "USD".into(),
            },
            LedgerPosting {
                account: "REALIZED_PNL".into(),
                amount_minor: -10,
                currency: "USD".into(),
            },
        ],
    }];
    let ledger_hash = ledger_commitment(&ledger);
    let metrics = ResultMetrics {
        survived: true,
        terminal_return_ppb: 1,
        max_drawdown_ppb: -2,
        peak_effective_leverage_ppb: 3,
        benchmark_return_ppb: 4,
    };
    let result_hash = result_commitment(
        "session-1",
        &manifest_set_hash,
        &event.current_event_hash,
        &state_hashes_hash,
        &ledger_hash,
        metrics,
    );

    format!(
        concat!(
            "{{",
            "\"verification_version\":\"1\",",
            "\"session_id\":\"session-1\",",
            "\"manifest_hashes\":[\"{}\"],",
            "\"manifest_set_hash\":\"{}\",",
            "\"inputs\":[{{\"session_id\":\"session-1\",\"input_seq\":\"0\",\"expected_state_version\":\"0\",\"logical_ts_ns\":\"1\",\"kind\":\"COMMAND_TEST\",\"payload_hex\":\"{}\"}}],",
            "\"kernel_events\":[{{\"event_seq\":\"0\",\"state_version\":\"1\",\"logical_ts_ns\":\"1\",\"kind\":\"COMMAND_TEST\",\"payload_hash\":\"{}\",\"prior_event_hash\":\"{}\",\"current_event_hash\":\"{}\"}}],",
            "\"state_hashes\":[{{\"event_seq\":\"0\",\"hash\":\"{}\"}}],",
            "\"state_hashes_hash\":\"{}\",",
            "\"ledger_transactions\":[{{\"event_seq\":\"0\",\"transaction_id\":\"tx-1\",\"postings\":[{{\"account\":\"CASH\",\"amount_minor\":\"10\",\"currency\":\"USD\"}},{{\"account\":\"REALIZED_PNL\",\"amount_minor\":\"-10\",\"currency\":\"USD\"}}]}}],",
            "\"ledger_hash\":\"{}\",",
            "\"metrics\":{{\"survived\":true,\"terminal_return_ppb\":\"1\",\"max_drawdown_ppb\":\"-2\",\"peak_effective_leverage_ppb\":\"3\",\"benchmark_return_ppb\":\"4\"}},",
            "\"result_hash\":\"{}\"",
            "}}"
        ),
        hash_hex(&manifests[0]),
        hash_hex(&manifest_set_hash),
        hex(&input.payload),
        hash_hex(&event.payload_hash),
        hash_hex(&event.prior_event_hash),
        hash_hex(&event.current_event_hash),
        hash_hex(&event.current_event_hash),
        hash_hex(&state_hashes_hash),
        hash_hex(&ledger_hash),
        hash_hex(&result_hash),
    )
}

#[test]
fn valid_bundle_reproduces_result_and_ledger() {
    let json = valid_bundle_json();
    let report = verify_bytes(json.as_bytes());
    assert!(report.valid, "{:?}", report.failure);
    assert_eq!(report.inputs_verified, 1);
    assert_eq!(report.ledger_transactions_verified, 1);
    let ledger = inspect_ledger_bytes(json.as_bytes()).unwrap();
    assert_eq!(ledger.transactions, 1);
    assert_eq!(
        ledger.balances.get(&("USD".into(), "CASH".into())),
        Some(&10)
    );
}

#[test]
fn tampered_command_identifies_event_mismatch() {
    let json = valid_bundle_json().replace(
        "\"payload_hex\":\"616263\"",
        "\"payload_hex\":\"616264\"",
    );
    let report = verify_bytes(json.as_bytes());
    assert_eq!(
        report.failure.as_ref().map(|failure| failure.code),
        Some(VerificationFailureCode::EventMismatch)
    );
    assert_eq!(
        report.failure.as_ref().and_then(|failure| failure.index),
        Some(0)
    );
}

#[test]
fn tampered_event_identifies_exact_event() {
    let json = valid_bundle_json();
    let marker = "\"current_event_hash\":\"";
    let start = json.find(marker).unwrap() + marker.len();
    let mut bytes = json.into_bytes();
    bytes[start] = if bytes[start] == b'a' { b'b' } else { b'a' };
    let report = verify_bytes(&bytes);
    assert_eq!(
        report.failure.as_ref().map(|failure| failure.code),
        Some(VerificationFailureCode::EventMismatch)
    );
    assert_eq!(
        report.failure.as_ref().and_then(|failure| failure.index),
        Some(0)
    );
}

#[test]
fn tampered_manifest_identifies_manifest_commitment() {
    let json = valid_bundle_json();
    let marker = "\"manifest_hashes\":[\"";
    let start = json.find(marker).unwrap() + marker.len();
    let mut bytes = json.into_bytes();
    bytes[start] = if bytes[start] == b'a' { b'b' } else { b'a' };
    let report = verify_bytes(&bytes);
    assert_eq!(
        report.failure.as_ref().map(|failure| failure.code),
        Some(VerificationFailureCode::ManifestCommitment)
    );
}

#[test]
fn unbalanced_ledger_fails_at_transaction() {
    let json = valid_bundle_json().replace(
        "\"amount_minor\":\"-10\"",
        "\"amount_minor\":\"-9\"",
    );
    let report = verify_bytes(json.as_bytes());
    assert_eq!(
        report.failure.as_ref().map(|failure| failure.code),
        Some(VerificationFailureCode::LedgerImbalance)
    );
    assert_eq!(
        report.failure.as_ref().and_then(|failure| failure.index),
        Some(0)
    );
}
