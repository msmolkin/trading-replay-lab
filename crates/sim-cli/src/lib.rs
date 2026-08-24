#![forbid(unsafe_code)]

//! Offline verification primitives for portable replay proof bundles.
//!
//! M3-08 exports a proof package containing the public result bundle plus the canonical kernel
//! inputs required to reproduce the simulator event-chain commitment. This crate verifies those
//! inputs without network access and keeps exact financial values as integers throughout.

mod json;

use std::collections::{BTreeMap, BTreeSet};

use json::JsonValue;
use sim_core::hash::{CanonicalWriter, Hash32, ZERO_HASH, hash_hex, sha256};
use sim_core::kernel::{InputEnvelope, Kernel, KernelEvent};

const PROOF_VERSION: &str = "1";
const HASH_HEX_LEN: usize = 64;

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
        code: VerificationFailureCode,
        index: Option<usize>,
        detail: impl Into<String>,
        inputs_verified: usize,
        ledger_transactions_verified: usize,
        final_event_hash: Option<String>,
    ) -> Self {
        Self {
            valid: false,
            failure: Some(VerificationFailure {
                code,
                index,
                detail: detail.into(),
            }),
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

/// Aggregated exact ledger balances for inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerInspection {
    /// Verified transaction count.
    pub transactions: usize,
    /// Exact balances keyed by `(currency, account)`.
    pub balances: BTreeMap<(String, String), i128>,
    /// Recomputed ledger commitment.
    pub ledger_hash: String,
}

impl LedgerInspection {
    /// Stable JSON for command-line inspection.
    #[must_use]
    pub fn to_json(&self) -> String {
        let balances = self
            .balances
            .iter()
            .map(|((currency, account), amount)| {
                format!(
                    "{{\"currency\":\"{}\",\"account\":\"{}\",\"amount_minor\":\"{}\"}}",
                    json_escape(currency),
                    json_escape(account),
                    amount
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"transactions\":{},\"ledger_hash\":\"{}\",\"balances\":[{}]}}",
            self.transactions, self.ledger_hash, balances
        )
    }
}

/// Verifies a complete proof bundle from UTF-8 JSON bytes.
#[must_use]
pub fn verify_bytes(bytes: &[u8]) -> VerificationReport {
    let root = match json::parse(bytes) {
        Ok(value) => value,
        Err(error) => {
            return VerificationReport::fail(
                VerificationFailureCode::Format,
                None,
                format!("JSON {:?} at byte {}", error.kind, error.offset),
                0,
                0,
                None,
            );
        }
    };
    let bundle = match ProofBundle::from_json(&root) {
        Ok(bundle) => bundle,
        Err(failure) => {
            return VerificationReport::fail(
                failure.code,
                failure.index,
                failure.detail,
                0,
                0,
                None,
            );
        }
    };
    verify_bundle(&bundle)
}

/// Inspects and validates only the balanced ledger section of a proof bundle.
pub fn inspect_ledger_bytes(bytes: &[u8]) -> Result<LedgerInspection, VerificationFailure> {
    let root = json::parse(bytes).map_err(|error| VerificationFailure {
        code: VerificationFailureCode::Format,
        index: None,
        detail: format!("JSON {:?} at byte {}", error.kind, error.offset),
    })?;
    let bundle = ProofBundle::from_json(&root)?;
    verify_ledger(&bundle.ledger_transactions, &bundle.ledger_hash)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProofBundle {
    session_id: String,
    manifest_hashes: Vec<Hash32>,
    manifest_set_hash: Hash32,
    inputs: Vec<InputEnvelope>,
    kernel_events: Vec<DeclaredKernelEvent>,
    state_hashes: Vec<StateHash>,
    state_hashes_hash: Hash32,
    ledger_transactions: Vec<LedgerTransaction>,
    ledger_hash: Hash32,
    metrics: ResultMetrics,
    result_hash: Hash32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DeclaredKernelEvent {
    event_seq: u64,
    state_version: u64,
    logical_ts_ns: i64,
    kind: String,
    payload_hash: Hash32,
    prior_event_hash: Hash32,
    current_event_hash: Hash32,
}

impl DeclaredKernelEvent {
    fn matches(&self, actual: &KernelEvent) -> bool {
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
struct StateHash {
    event_seq: u64,
    hash: Hash32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LedgerTransaction {
    event_seq: u64,
    transaction_id: String,
    postings: Vec<LedgerPosting>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct LedgerPosting {
    account: String,
    amount_minor: i64,
    currency: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ResultMetrics {
    survived: bool,
    terminal_return_ppb: i64,
    max_drawdown_ppb: i64,
    peak_effective_leverage_ppb: i64,
    benchmark_return_ppb: i64,
}

impl ProofBundle {
    fn from_json(value: &JsonValue) -> Result<Self, VerificationFailure> {
        let root = object(value, "proof bundle")?;
        if text(required(root, "verification_version")?, "verification_version")? != PROOF_VERSION {
            return Err(format_failure("unsupported verification_version"));
        }
        let session_id = nonempty_text(required(root, "session_id")?, "session_id")?.to_owned();
        let manifest_hashes = parse_hash_array(required(root, "manifest_hashes")?, "manifest_hashes")?;
        let manifest_set_hash = hash_value(required(root, "manifest_set_hash")?, "manifest_set_hash")?;
        let inputs = parse_inputs(required(root, "inputs")?)?;
        let kernel_events = parse_kernel_events(required(root, "kernel_events")?)?;
        let state_hashes = parse_state_hashes(required(root, "state_hashes")?)?;
        let state_hashes_hash = hash_value(required(root, "state_hashes_hash")?, "state_hashes_hash")?;
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

fn verify_bundle(bundle: &ProofBundle) -> VerificationReport {
    let manifest_hash = manifest_commitment(&bundle.manifest_hashes);
    if manifest_hash != bundle.manifest_set_hash {
        return VerificationReport::fail(
            VerificationFailureCode::ManifestCommitment,
            None,
            "manifest set commitment does not match manifest_hashes",
            0,
            0,
            None,
        );
    }

    if bundle.inputs.len() != bundle.kernel_events.len() {
        return VerificationReport::fail(
            VerificationFailureCode::EventMismatch,
            None,
            "inputs and kernel_events must have equal length",
            0,
            0,
            None,
        );
    }
    let mut kernel = Kernel::new();
    let mut verified = 0_usize;
    let mut final_hash = ZERO_HASH;
    for (index, (input, declared)) in bundle
        .inputs
        .iter()
        .zip(&bundle.kernel_events)
        .enumerate()
    {
        if input.session_id != bundle.session_id {
            return VerificationReport::fail(
                VerificationFailureCode::InputSequence,
                Some(index),
                "input session_id differs from proof session_id",
                verified,
                0,
                Some(hash_hex(&final_hash)),
            );
        }
        let actual = match kernel.apply(input) {
            Ok(event) => event,
            Err(error) => {
                return VerificationReport::fail(
                    VerificationFailureCode::InputSequence,
                    Some(index),
                    error.to_string(),
                    verified,
                    0,
                    Some(hash_hex(&final_hash)),
                );
            }
        };
        if !declared.matches(&actual) {
            return VerificationReport::fail(
                VerificationFailureCode::EventMismatch,
                Some(index),
                format!("declared kernel event {} differs from reproduced event", declared.event_seq),
                verified,
                0,
                Some(hash_hex(&final_hash)),
            );
        }
        final_hash = actual.current_event_hash;
        verified += 1;
    }

    if let Err(failure) = verify_state_hashes(&bundle.state_hashes, &bundle.state_hashes_hash) {
        return VerificationReport::fail(
            failure.code,
            failure.index,
            failure.detail,
            verified,
            0,
            Some(hash_hex(&final_hash)),
        );
    }
    let state_commitment = state_hash_commitment(&bundle.state_hashes);

    let ledger = match verify_ledger(&bundle.ledger_transactions, &bundle.ledger_hash) {
        Ok(ledger) => ledger,
        Err(failure) => {
            return VerificationReport::fail(
                failure.code,
                failure.index,
                failure.detail,
                verified,
                0,
                Some(hash_hex(&final_hash)),
            );
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
            VerificationFailureCode::ResultCommitment,
            None,
            "result_hash does not match reproduced proof commitments and metrics",
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

fn verify_state_hashes(
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

fn verify_ledger(
    transactions: &[LedgerTransaction],
    declared_hash: &Hash32,
) -> Result<LedgerInspection, VerificationFailure> {
    let mut seen_ids = BTreeSet::new();
    let mut balances = BTreeMap::<(String, String), i128>::new();
    let mut prior_event_seq = None;
    for (index, transaction) in transactions.iter().enumerate() {
        if transaction.postings.len() < 2 {
            return Err(VerificationFailure {
                code: VerificationFailureCode::LedgerFormat,
                index: Some(index),
                detail: "ledger transaction requires at least two postings".into(),
            });
        }
        if !seen_ids.insert(transaction.transaction_id.clone()) {
            return Err(VerificationFailure {
                code: VerificationFailureCode::LedgerFormat,
                index: Some(index),
                detail: "duplicate ledger transaction_id".into(),
            });
        }
        if prior_event_seq.is_some_and(|value| transaction.event_seq < value) {
            return Err(VerificationFailure {
                code: VerificationFailureCode::LedgerFormat,
                index: Some(index),
                detail: "ledger transactions must be ordered by event_seq".into(),
            });
        }
        prior_event_seq = Some(transaction.event_seq);
        let mut currency_totals = BTreeMap::<String, i128>::new();
        for posting in &transaction.postings {
            *currency_totals.entry(posting.currency.clone()).or_default() +=
                i128::from(posting.amount_minor);
            *balances
                .entry((posting.currency.clone(), posting.account.clone()))
                .or_default() += i128::from(posting.amount_minor);
        }
        if let Some((currency, amount)) = currency_totals.iter().find(|(_, amount)| **amount != 0) {
            return Err(VerificationFailure {
                code: VerificationFailureCode::LedgerImbalance,
                index: Some(index),
                detail: format!(
                    "transaction {} is unbalanced in {} by {} minor units",
                    transaction.transaction_id, currency, amount
                ),
            });
        }
    }
    let computed = ledger_commitment(transactions);
    if computed != *declared_hash {
        return Err(VerificationFailure {
            code: VerificationFailureCode::LedgerCommitment,
            index: None,
            detail: "ledger_hash does not match ledger_transactions".into(),
        });
    }
    Ok(LedgerInspection {
        transactions: transactions.len(),
        balances,
        ledger_hash: hash_hex(&computed),
    })
}

fn manifest_commitment(hashes: &[Hash32]) -> Hash32 {
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

fn state_hash_commitment(states: &[StateHash]) -> Hash32 {
    let mut writer = CanonicalWriter::new();
    writer.tag(b"TRL-STATE-HASHES-v1\0");
    writer.u64(u64::try_from(states.len()).expect("in-memory state list exceeds u64"));
    for state in states {
        writer.u64(state.event_seq);
        writer.hash(&state.hash);
    }
    sha256(&writer.finish())
}

fn ledger_commitment(transactions: &[LedgerTransaction]) -> Hash32 {
    let mut writer = CanonicalWriter::new();
    writer.tag(b"TRL-LEDGER-PROOF-v1\0");
    writer.u64(u64::try_from(transactions.len()).expect("in-memory ledger exceeds u64"));
    for transaction in transactions {
        writer.u64(transaction.event_seq);
        writer.text(&transaction.transaction_id);
        let mut postings = transaction.postings.clone();
        postings.sort();
        writer.u64(u64::try_from(postings.len()).expect("in-memory postings exceed u64"));
        for posting in postings {
            writer.text(&posting.currency);
            writer.text(&posting.account);
            writer.i64(posting.amount_minor);
        }
    }
    sha256(&writer.finish())
}

fn result_commitment(
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
    writer.u64(u64::from(metrics.survived));
    writer.i64(metrics.terminal_return_ppb);
    writer.i64(metrics.max_drawdown_ppb);
    writer.i64(metrics.peak_effective_leverage_ppb);
    writer.i64(metrics.benchmark_return_ppb);
    sha256(&writer.finish())
}

fn parse_inputs(value: &JsonValue) -> Result<Vec<InputEnvelope>, VerificationFailure> {
    array(value, "inputs")?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let fields = object(value, "input").map_err(|failure| with_index(failure, index))?;
            let session_id = nonempty_text(required(fields, "session_id")?, "session_id")?.to_owned();
            let input_seq = decimal_u64(required(fields, "input_seq")?, "input_seq")?;
            let expected_state_version = decimal_u64(
                required(fields, "expected_state_version")?,
                "expected_state_version",
            )?;
            let logical_ts_ns = decimal_i64(required(fields, "logical_ts_ns")?, "logical_ts_ns")?;
            let kind = nonempty_text(required(fields, "kind")?, "kind")?.to_owned();
            let payload = decode_hex(text(required(fields, "payload_hex")?, "payload_hex")?)
                .map_err(|detail| VerificationFailure {
                    code: VerificationFailureCode::Format,
                    index: Some(index),
                    detail,
                })?;
            Ok(InputEnvelope {
                session_id,
                input_seq,
                expected_state_version,
                logical_ts_ns,
                kind,
                payload,
            })
        })
        .collect()
}

fn parse_kernel_events(value: &JsonValue) -> Result<Vec<DeclaredKernelEvent>, VerificationFailure> {
    array(value, "kernel_events")?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let fields = object(value, "kernel_event").map_err(|failure| with_index(failure, index))?;
            Ok(DeclaredKernelEvent {
                event_seq: decimal_u64(required(fields, "event_seq")?, "event_seq")?,
                state_version: decimal_u64(required(fields, "state_version")?, "state_version")?,
                logical_ts_ns: decimal_i64(required(fields, "logical_ts_ns")?, "logical_ts_ns")?,
                kind: nonempty_text(required(fields, "kind")?, "kind")?.to_owned(),
                payload_hash: hash_value(required(fields, "payload_hash")?, "payload_hash")?,
                prior_event_hash: hash_value(
                    required(fields, "prior_event_hash")?,
                    "prior_event_hash",
                )?,
                current_event_hash: hash_value(
                    required(fields, "current_event_hash")?,
                    "current_event_hash",
                )?,
            })
        })
        .collect()
}

fn parse_state_hashes(value: &JsonValue) -> Result<Vec<StateHash>, VerificationFailure> {
    array(value, "state_hashes")?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let fields = object(value, "state_hash").map_err(|failure| with_index(failure, index))?;
            Ok(StateHash {
                event_seq: decimal_u64(required(fields, "event_seq")?, "event_seq")?,
                hash: hash_value(required(fields, "hash")?, "hash")?,
            })
        })
        .collect()
}

fn parse_ledger(value: &JsonValue) -> Result<Vec<LedgerTransaction>, VerificationFailure> {
    array(value, "ledger_transactions")?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let fields = object(value, "ledger_transaction")
                .map_err(|failure| with_index(failure, index))?;
            let postings = array(required(fields, "postings")?, "postings")?
                .iter()
                .map(|posting| {
                    let fields = object(posting, "ledger posting")?;
                    Ok(LedgerPosting {
                        account: nonempty_text(required(fields, "account")?, "account")?.to_owned(),
                        amount_minor: decimal_i64(
                            required(fields, "amount_minor")?,
                            "amount_minor",
                        )?,
                        currency: nonempty_text(required(fields, "currency")?, "currency")?
                            .to_owned(),
                    })
                })
                .collect::<Result<Vec<_>, VerificationFailure>>()?;
            Ok(LedgerTransaction {
                event_seq: decimal_u64(required(fields, "event_seq")?, "event_seq")?,
                transaction_id: nonempty_text(
                    required(fields, "transaction_id")?,
                    "transaction_id",
                )?
                .to_owned(),
                postings,
            })
        })
        .collect()
}

fn parse_metrics(value: &JsonValue) -> Result<ResultMetrics, VerificationFailure> {
    let fields = object(value, "metrics")?;
    Ok(ResultMetrics {
        survived: boolean(required(fields, "survived")?, "survived")?,
        terminal_return_ppb: decimal_i64(
            required(fields, "terminal_return_ppb")?,
            "terminal_return_ppb",
        )?,
        max_drawdown_ppb: decimal_i64(
            required(fields, "max_drawdown_ppb")?,
            "max_drawdown_ppb",
        )?,
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
    if raw.len() != HASH_HEX_LEN || !raw.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()) {
        return Err(format_failure(format!("{name} must be lowercase SHA-256 hex")));
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
        return Err(format_failure(format!("{name} must be canonical unsigned decimal text")));
    }
    Ok(())
}

fn canonical_signed(raw: &str, name: &str) -> Result<(), VerificationFailure> {
    if raw.is_empty() || raw.starts_with('+') || raw == "-0" {
        return Err(format_failure(format!("{name} must be canonical signed decimal text")));
    }
    let digits = raw.strip_prefix('-').unwrap_or(raw);
    if digits.is_empty()
        || (digits.starts_with('0') && digits != "0")
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format_failure(format!("{name} must be canonical signed decimal text")));
    }
    Ok(())
}

fn decode_hex(raw: &str) -> Result<Vec<u8>, String> {
    if raw.len() % 2 != 0 {
        return Err("hex value must have even length".into());
    }
    let mut output = Vec::with_capacity(raw.len() / 2);
    for pair in raw.as_bytes().chunks_exact(2) {
        let high = hex_nibble(pair[0]).ok_or_else(|| "hex value contains invalid digit".to_owned())?;
        let low = hex_nibble(pair[1]).ok_or_else(|| "hex value contains invalid digit".to_owned())?;
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

fn json_option_string(value: Option<&str>) -> String {
    value.map_or_else(
        || "null".to_owned(),
        |value| format!("\"{}\"", json_escape(value)),
    )
}

fn json_escape(value: &str) -> String {
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
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
        let json = valid_bundle_json().replace("\"payload_hex\":\"616263\"", "\"payload_hex\":\"616264\"");
        let report = verify_bytes(json.as_bytes());
        assert_eq!(
            report.failure.as_ref().map(|failure| failure.code),
            Some(VerificationFailureCode::EventMismatch)
        );
        assert_eq!(report.failure.as_ref().and_then(|failure| failure.index), Some(0));
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
        assert_eq!(report.failure.as_ref().and_then(|failure| failure.index), Some(0));
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
        let json = valid_bundle_json().replace("\"amount_minor\":\"-10\"", "\"amount_minor\":\"-9\"");
        let report = verify_bytes(json.as_bytes());
        assert_eq!(
            report.failure.as_ref().map(|failure| failure.code),
            Some(VerificationFailureCode::LedgerImbalance)
        );
        assert_eq!(report.failure.as_ref().and_then(|failure| failure.index), Some(0));
    }
}
