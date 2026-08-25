use std::collections::{BTreeMap, BTreeSet};

use sim_core::hash::{CanonicalWriter, Hash32, hash_hex, sha256};

use crate::{VerificationFailure, VerificationFailureCode, json_escape};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LedgerTransaction {
    pub event_seq: u64,
    pub transaction_id: String,
    pub postings: Vec<LedgerPosting>,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct LedgerPosting {
    pub account: String,
    pub amount_minor: i64,
    pub currency: String,
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

pub(crate) fn verify_ledger(
    transactions: &[LedgerTransaction],
    declared_hash: &Hash32,
) -> Result<LedgerInspection, VerificationFailure> {
    let mut seen_ids = BTreeSet::new();
    let mut balances = BTreeMap::<(String, String), i128>::new();
    let mut prior_event_seq = None;
    for (index, transaction) in transactions.iter().enumerate() {
        validate_transaction_shape(transaction, index, &mut seen_ids, prior_event_seq)?;
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

fn validate_transaction_shape(
    transaction: &LedgerTransaction,
    index: usize,
    seen_ids: &mut BTreeSet<String>,
    prior_event_seq: Option<u64>,
) -> Result<(), VerificationFailure> {
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
    Ok(())
}

pub(crate) fn ledger_commitment(transactions: &[LedgerTransaction]) -> Hash32 {
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
