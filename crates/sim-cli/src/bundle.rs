use std::collections::BTreeMap;

use sim_core::hash::{CanonicalWriter, Hash32, ZERO_HASH, sha256};
use sim_core::kernel::{InputEnvelope, KernelEvent};

use crate::json::JsonValue;
use crate::ledger::{LedgerPosting, LedgerTransaction};
use crate::{VerificationFailure, VerificationFailureCode};

const PROOF_VERSION: &str = "1";
const HASH_HEX_LEN: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProofBundle {
    pub session_id: String,
    pub manifest_hashes: Vec<Hash32>,
    pub manifest_set_hash: Hash32,
    pub inputs: Vec<InputEnvelope>,
    pub kernel_events: Vec<DeclaredKernelEvent>,
    pub state_hashes: Vec<StateHash>,
    pub state_hashes_hash: Hash32,
    pub ledger_transactions: Vec<LedgerTransaction>,
    pub ledger_hash: Hash32,
    pub metrics: ResultMetrics,
    pub result_hash: Hash32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeclaredKernelEvent {
    pub event_seq: u64,
    pub state_version: u64,
    pub logical_ts_ns: i64,
    pub kind: String,
    pub payload_hash: Hash32,
    pub prior_event_hash: Hash32,
    pub current_event_hash: Hash32,
}

impl DeclaredKernelEvent {
    pub fn matches(&self, actual: &KernelEvent) -> bool {
        self.event_seq == actual.event_seq
            && self.state_version == actual.state_version
            && self.logical_ts_ns == actual.logical_ts_ns
            && self.kind == actual.kind
            && self.payload_hash == actual.payload_hash
            && self.prior_event_hash == actual.prior_event_hash
            && self.current_event_hash == actual.current_event_hash
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StateHash {
    pub event_seq: u64,
    pub hash: Hash32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResultMetrics {
    pub survived: bool,
    pub terminal_return_ppb: i64,
    pub max_drawdown_ppb: i64,
    pub peak_effective_leverage_ppb: i64,
    pub benchmark_return_ppb: i64,
}

impl ProofBundle {
    pub fn from_json(value: &JsonValue) -> Result<Self, VerificationFailure> {
        let root = object(value, "proof bundle")?;
        let version = text(
            required(root, "verification_version")?,
            "verification_version",
        )?;
        if version != PROOF_VERSION {
            return Err(format_failure("unsupported verification_version"));
        }
        let session_id = nonempty_text(required(root, "session_id")?, "session_id")?.to_owned();
        let manifest_hashes =
            parse_hash_array(required(root, "manifest_hashes")?, "manifest_hashes")?;
        let manifest_set_hash =
            hash_value(required(root, "manifest_set_hash")?, "manifest_set_hash")?;
        let inputs = parse_inputs(required(root, "inputs")?)?;
        let kernel_events = parse_kernel_events(required(root, "kernel_events")?)?;
        let state_hashes = parse_state_hashes(required(root, "state_hashes")?)?;
        let state_hashes_hash =
            hash_value(required(root, "state_hashes_hash")?, "state_hashes_hash")?;
        let ledger_transactions = parse_ledger(required(root, "ledger_transactions")?)?;
        let ledger_hash = hash_value(required(root, "ledger_hash")?, "ledger_hash")?;
        let metrics = parse_metrics(required(root, "metrics")?)?;
        let result_hash = hash_value(required(root, "result_hash")?, "result_hash")?;
        Ok(Self {
            session_id,
            manifest_hashes,
            manifest_set_hash,
            inputs,
            kernel_events,
            state_hashes,
            state_hashes_hash,
            ledger_transactions,
            ledger_hash,
            metrics,
            result_hash,
        })
    }
}

pub(crate) fn manifest_commitment(hashes: &[Hash32]) -> Hash32 {
    let mut sorted = hashes.to_vec();
    sorted.sort_unstable();
    let mut writer = CanonicalWriter::new();
    writer.tag(b"TRL-MANIFEST-SET-v1\0");
    writer.u64(u64::try_from(sorted.len()).expect("in-memory manifest list exceeds u64"));
    for hash in sorted {
        writer.hash(&hash);
    }
    sha256(&writer.finish())
}

pub(crate) fn state_hash_commitment(states: &[StateHash]) -> Hash32 {
    let mut writer = CanonicalWriter::new();
    writer.tag(b"TRL-STATE-HASHES-v1\0");
    writer.u64(u64::try_from(states.len()).expect("in-memory state list exceeds u64"));
    for state in states {
        writer.u64(state.event_seq);
        writer.hash(&state.hash);
    }
    sha256(&writer.finish())
}

pub(crate) fn result_commitment(
    session_id: &str,
    manifest_set_hash: &Hash32,
    final_event_hash: &Hash32,
    state_hashes_hash: &Hash32,
    ledger_hash: &Hash32,
    metrics: ResultMetrics,
) -> Hash32 {
    let mut writer = CanonicalWriter::new();
    writer.tag(b"TRL-VERIFIER-RESULT-v1\0");
    writer.text(session_id);
    writer.hash(manifest_set_hash);
    writer.hash(final_event_hash);
    writer.hash(state_hashes_hash);
    writer.hash(ledger_hash);
    writer.u64(if metrics.survived { 1 } else { 0 });
    writer.i64(metrics.terminal_return_ppb);
    writer.i64(metrics.max_drawdown_ppb);
    writer.i64(metrics.peak_effective_leverage_ppb);
    writer.i64(metrics.benchmark_return_ppb);
    sha256(&writer.finish())
}

pub(crate) fn verify_state_hashes(
    state_hashes: &[StateHash],
    declared_commitment: &Hash32,
) -> Result<(), VerificationFailure> {
    let mut prior = None;
    for (index, state) in state_hashes.iter().enumerate() {
        if prior.is_some_and(|value| state.event_seq <= value) {
            return Err(VerificationFailure {
                code: VerificationFailureCode::StateHash,
                index: Some(index),
                detail: "state_hashes event_seq must be strictly ascending".into(),
            });
        }
        prior = Some(state.event_seq);
    }
    if state_hash_commitment(state_hashes) != *declared_commitment {
        return Err(VerificationFailure {
            code: VerificationFailureCode::StateCommitment,
            index: None,
            detail: "state_hashes_hash does not match state_hashes".into(),
        });
    }
    Ok(())
}

fn parse_inputs(value: &JsonValue) -> Result<Vec<InputEnvelope>, VerificationFailure> {
    array(value, "inputs")?
        .iter()
        .enumerate()
        .map(|(index, value)| parse_input(value).map_err(|failure| with_index(failure, index)))
        .collect()
}

fn parse_input(value: &JsonValue) -> Result<InputEnvelope, VerificationFailure> {
    let fields = object(value, "input")?;
    let session_id = nonempty_text(required(fields, "session_id")?, "session_id")?.to_owned();
    let input_seq = decimal_u64(required(fields, "input_seq")?, "input_seq")?;
    let expected_state_version = decimal_u64(
        required(fields, "expected_state_version")?,
        "expected_state_version",
    )?;
    let logical_ts_ns = decimal_i64(required(fields, "logical_ts_ns")?, "logical_ts_ns")?;
    let kind = nonempty_text(required(fields, "kind")?, "kind")?.to_owned();
    let payload = decode_hex(text(required(fields, "payload_hex")?, "payload_hex")?)
        .map_err(format_failure)?;
    Ok(InputEnvelope {
        session_id,
        input_seq,
        expected_state_version,
        logical_ts_ns,
        kind,
        payload,
    })
}

fn parse_kernel_events(value: &JsonValue) -> Result<Vec<DeclaredKernelEvent>, VerificationFailure> {
    array(value, "kernel_events")?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            parse_kernel_event(value).map_err(|failure| with_index(failure, index))
        })
        .collect()
}

fn parse_kernel_event(value: &JsonValue) -> Result<DeclaredKernelEvent, VerificationFailure> {
    let fields = object(value, "kernel_event")?;
    Ok(DeclaredKernelEvent {
        event_seq: decimal_u64(required(fields, "event_seq")?, "event_seq")?,
        state_version: decimal_u64(required(fields, "state_version")?, "state_version")?,
        logical_ts_ns: decimal_i64(required(fields, "logical_ts_ns")?, "logical_ts_ns")?,
        kind: nonempty_text(required(fields, "kind")?, "kind")?.to_owned(),
        payload_hash: hash_value(required(fields, "payload_hash")?, "payload_hash")?,
        prior_event_hash: hash_value(required(fields, "prior_event_hash")?, "prior_event_hash")?,
        current_event_hash: hash_value(
            required(fields, "current_event_hash")?,
            "current_event_hash",
        )?,
    })
}

fn parse_state_hashes(value: &JsonValue) -> Result<Vec<StateHash>, VerificationFailure> {
    array(value, "state_hashes")?
        .iter()
        .enumerate()
        .map(|(index, value)| parse_state_hash(value).map_err(|failure| with_index(failure, index)))
        .collect()
}

fn parse_state_hash(value: &JsonValue) -> Result<StateHash, VerificationFailure> {
    let fields = object(value, "state_hash")?;
    Ok(StateHash {
        event_seq: decimal_u64(required(fields, "event_seq")?, "event_seq")?,
        hash: hash_value(required(fields, "hash")?, "hash")?,
    })
}

fn parse_ledger(value: &JsonValue) -> Result<Vec<LedgerTransaction>, VerificationFailure> {
    array(value, "ledger_transactions")?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            parse_ledger_transaction(value).map_err(|failure| with_index(failure, index))
        })
        .collect()
}

fn parse_ledger_transaction(value: &JsonValue) -> Result<LedgerTransaction, VerificationFailure> {
    let fields = object(value, "ledger_transaction")?;
    let postings = array(required(fields, "postings")?, "postings")?
        .iter()
        .map(parse_ledger_posting)
        .collect::<Result<Vec<_>, VerificationFailure>>()?;
    Ok(LedgerTransaction {
        event_seq: decimal_u64(required(fields, "event_seq")?, "event_seq")?,
        transaction_id: nonempty_text(required(fields, "transaction_id")?, "transaction_id")?
            .to_owned(),
        postings,
    })
}

fn parse_ledger_posting(value: &JsonValue) -> Result<LedgerPosting, VerificationFailure> {
    let fields = object(value, "ledger posting")?;
    Ok(LedgerPosting {
        account: nonempty_text(required(fields, "account")?, "account")?.to_owned(),
        amount_minor: decimal_i64(required(fields, "amount_minor")?, "amount_minor")?,
        currency: nonempty_text(required(fields, "currency")?, "currency")?.to_owned(),
    })
}

fn parse_metrics(value: &JsonValue) -> Result<ResultMetrics, VerificationFailure> {
    let fields = object(value, "metrics")?;
    Ok(ResultMetrics {
        survived: boolean(required(fields, "survived")?, "survived")?,
        terminal_return_ppb: decimal_i64(
            required(fields, "terminal_return_ppb")?,
            "terminal_return_ppb",
        )?,
        max_drawdown_ppb: decimal_i64(required(fields, "max_drawdown_ppb")?, "max_drawdown_ppb")?,
        peak_effective_leverage_ppb: decimal_i64(
            required(fields, "peak_effective_leverage_ppb")?,
            "peak_effective_leverage_ppb",
        )?,
        benchmark_return_ppb: decimal_i64(
            required(fields, "benchmark_return_ppb")?,
            "benchmark_return_ppb",
        )?,
    })
}

fn parse_hash_array(value: &JsonValue, name: &str) -> Result<Vec<Hash32>, VerificationFailure> {
    array(value, name)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            hash_value(value, name).map_err(|mut failure| {
                failure.code = VerificationFailureCode::ManifestHash;
                failure.index = Some(index);
                failure
            })
        })
        .collect()
}

fn hash_value(value: &JsonValue, name: &str) -> Result<Hash32, VerificationFailure> {
    let raw = text(value, name)?;
    if raw.len() != HASH_HEX_LEN
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(format_failure(format!(
            "{name} must be lowercase SHA-256 hex"
        )));
    }
    let bytes = decode_hex(raw).map_err(format_failure)?;
    bytes
        .try_into()
        .map_err(|_| format_failure(format!("{name} must contain 32 bytes")))
}

fn decimal_u64(value: &JsonValue, name: &str) -> Result<u64, VerificationFailure> {
    let raw = text(value, name)?;
    canonical_unsigned(raw, name)?;
    raw.parse::<u64>()
        .map_err(|_| format_failure(format!("{name} exceeds u64")))
}

fn decimal_i64(value: &JsonValue, name: &str) -> Result<i64, VerificationFailure> {
    let raw = text(value, name)?;
    canonical_signed(raw, name)?;
    raw.parse::<i64>()
        .map_err(|_| format_failure(format!("{name} exceeds i64")))
}

fn canonical_unsigned(raw: &str, name: &str) -> Result<(), VerificationFailure> {
    if raw.is_empty()
        || raw.starts_with('+')
        || raw.starts_with('-')
        || (raw.starts_with('0') && raw != "0")
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format_failure(format!(
            "{name} must be canonical unsigned decimal text"
        )));
    }
    Ok(())
}

fn canonical_signed(raw: &str, name: &str) -> Result<(), VerificationFailure> {
    if raw.is_empty() || raw.starts_with('+') || raw == "-0" {
        return Err(format_failure(format!(
            "{name} must be canonical signed decimal text"
        )));
    }
    let digits = raw.strip_prefix('-').unwrap_or(raw);
    if digits.is_empty()
        || (digits.starts_with('0') && digits != "0")
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format_failure(format!(
            "{name} must be canonical signed decimal text"
        )));
    }
    Ok(())
}

fn decode_hex(raw: &str) -> Result<Vec<u8>, String> {
    if raw.len() % 2 != 0 {
        return Err("hex value must have even length".into());
    }
    let mut output = Vec::with_capacity(raw.len() / 2);
    for pair in raw.as_bytes().chunks_exact(2) {
        let high =
            hex_nibble(pair[0]).ok_or_else(|| "hex value contains invalid digit".to_owned())?;
        let low =
            hex_nibble(pair[1]).ok_or_else(|| "hex value contains invalid digit".to_owned())?;
        output.push((high << 4) | low);
    }
    Ok(output)
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn required<'a>(
    fields: &'a BTreeMap<String, JsonValue>,
    name: &str,
) -> Result<&'a JsonValue, VerificationFailure> {
    fields
        .get(name)
        .ok_or_else(|| format_failure(format!("missing required field {name}")))
}

fn object<'a>(
    value: &'a JsonValue,
    name: &str,
) -> Result<&'a BTreeMap<String, JsonValue>, VerificationFailure> {
    match value {
        JsonValue::Object(fields) => Ok(fields),
        _ => Err(format_failure(format!("{name} must be an object"))),
    }
}

fn array<'a>(value: &'a JsonValue, name: &str) -> Result<&'a [JsonValue], VerificationFailure> {
    match value {
        JsonValue::Array(values) => Ok(values),
        _ => Err(format_failure(format!("{name} must be an array"))),
    }
}

fn text<'a>(value: &'a JsonValue, name: &str) -> Result<&'a str, VerificationFailure> {
    match value {
        JsonValue::String(value) => Ok(value),
        _ => Err(format_failure(format!("{name} must be a string"))),
    }
}

fn nonempty_text<'a>(value: &'a JsonValue, name: &str) -> Result<&'a str, VerificationFailure> {
    let value = text(value, name)?;
    if value.is_empty() {
        return Err(format_failure(format!("{name} cannot be empty")));
    }
    Ok(value)
}

fn boolean(value: &JsonValue, name: &str) -> Result<bool, VerificationFailure> {
    match value {
        JsonValue::Bool(value) => Ok(*value),
        _ => Err(format_failure(format!("{name} must be boolean"))),
    }
}

fn format_failure(detail: impl Into<String>) -> VerificationFailure {
    VerificationFailure {
        code: VerificationFailureCode::Format,
        index: None,
        detail: detail.into(),
    }
}

fn with_index(mut failure: VerificationFailure, index: usize) -> VerificationFailure {
    failure.index = Some(index);
    failure
}

#[cfg(test)]
pub(crate) fn zero_hash() -> Hash32 {
    ZERO_HASH
}