//! Checked double-entry ledger for deterministic simulator economics.

use std::collections::BTreeMap;

use crate::numeric::MoneyMinor;

/// Stable economic account buckets used by simulator postings.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum LedgerAccount {
    /// Settlement cash or cash-equivalent balance.
    Cash,
    /// Position cost/basis clearing account.
    PositionCost,
    /// Realized trading P&L.
    RealizedPnl,
    /// Trading fees and rebates.
    Fees,
    /// Funding payments.
    Funding,
    /// Borrow charges.
    Borrow,
    /// Dividends and distributions.
    Dividends,
    /// Settlement/expiry adjustments.
    Settlement,
}

/// One signed posting in settlement minor units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Posting {
    /// Ledger account to debit/credit.
    pub account: LedgerAccount,
    /// Signed posting amount.
    pub amount: MoneyMinor,
}

/// Immutable committed transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerTransaction {
    /// Monotonic simulator-owned transaction identifier.
    pub transaction_id: u64,
    /// Logical event sequence that caused the transaction.
    pub event_seq: u64,
    /// Stable transaction kind identifier.
    pub kind: String,
    /// Balanced postings in caller-supplied canonical order.
    pub postings: Vec<Posting>,
}

/// Proposed transaction before a simulator-owned identifier is assigned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewTransaction {
    /// Logical event sequence that caused the transaction.
    pub event_seq: u64,
    /// Stable transaction kind identifier.
    pub kind: String,
    /// Postings that must sum to exactly zero.
    pub postings: Vec<Posting>,
}

/// Serializable current ledger state used by simulator snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LedgerSnapshot {
    /// Next transaction identifier.
    pub next_transaction_id: u64,
    /// Current balances in deterministic account order.
    pub balances: Vec<(LedgerAccount, MoneyMinor)>,
}

/// Stable fail-closed ledger errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LedgerError {
    /// A double-entry transaction requires at least two postings.
    TooFewPostings,
    /// Transaction kind cannot be empty.
    EmptyKind,
    /// Postings do not sum exactly to zero.
    Unbalanced,
    /// Checked balance/sum/id arithmetic overflowed.
    Overflow,
    /// Snapshot contains duplicate accounts or a non-zero aggregate balance.
    InvalidSnapshot,
}

impl core::fmt::Display for LedgerError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooFewPostings => {
                formatter.write_str("ledger transaction needs at least two postings")
            }
            Self::EmptyKind => formatter.write_str("ledger transaction kind cannot be empty"),
            Self::Unbalanced => formatter.write_str("ledger transaction is not balanced"),
            Self::Overflow => formatter.write_str("ledger arithmetic overflow"),
            Self::InvalidSnapshot => formatter.write_str("invalid ledger snapshot"),
        }
    }
}

impl std::error::Error for LedgerError {}

/// Deterministic append-only double-entry ledger.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ledger {
    next_transaction_id: u64,
    balances: BTreeMap<LedgerAccount, MoneyMinor>,
    transactions: Vec<LedgerTransaction>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

impl Ledger {
    /// Creates an empty balanced ledger.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_transaction_id: 0,
            balances: BTreeMap::new(),
            transactions: Vec::new(),
        }
    }

    /// Restores current economic balances after validating snapshot invariants.
    ///
    /// Historical transactions are intentionally not reconstructed by this economic
    /// snapshot; authoritative history remains in the domain-event stream.
    ///
    /// # Errors
    /// Returns [`LedgerError::InvalidSnapshot`] when accounts repeat or aggregate balance
    /// is non-zero, and [`LedgerError::Overflow`] when validation cannot sum safely.
    pub fn from_snapshot(snapshot: LedgerSnapshot) -> Result<Self, LedgerError> {
        let mut balances = BTreeMap::new();
        let mut aggregate = 0_i64;
        for (account, balance) in snapshot.balances {
            if balances.insert(account, balance).is_some() {
                return Err(LedgerError::InvalidSnapshot);
            }
            aggregate = aggregate
                .checked_add(balance.get())
                .ok_or(LedgerError::Overflow)?;
        }
        if aggregate != 0 {
            return Err(LedgerError::InvalidSnapshot);
        }
        Ok(Self {
            next_transaction_id: snapshot.next_transaction_id,
            balances,
            transactions: Vec::new(),
        })
    }

    /// Returns one current account balance, defaulting to zero.
    #[must_use]
    pub fn balance(&self, account: LedgerAccount) -> MoneyMinor {
        self.balances
            .get(&account)
            .copied()
            .unwrap_or(MoneyMinor::new(0))
    }

    /// Iterates committed transactions in identifier order.
    pub fn transactions(&self) -> impl Iterator<Item = &LedgerTransaction> {
        self.transactions.iter()
    }

    /// Captures current balances for deterministic simulator restore.
    #[must_use]
    pub fn snapshot(&self) -> LedgerSnapshot {
        LedgerSnapshot {
            next_transaction_id: self.next_transaction_id,
            balances: self
                .balances
                .iter()
                .map(|(account, balance)| (*account, *balance))
                .collect(),
        }
    }

    /// Validates and atomically commits one exactly balanced transaction.
    ///
    /// All sums, resulting balances, and the next identifier are computed before the
    /// ledger mutates, so any failure leaves balances/history unchanged.
    ///
    /// # Errors
    /// Returns a stable structural, balancing, or overflow error without mutation.
    pub fn record(&mut self, transaction: NewTransaction) -> Result<u64, LedgerError> {
        if transaction.postings.len() < 2 {
            return Err(LedgerError::TooFewPostings);
        }
        if transaction.kind.is_empty() {
            return Err(LedgerError::EmptyKind);
        }

        let mut sum = 0_i64;
        let mut next_balances = self.balances.clone();
        for posting in &transaction.postings {
            sum = sum
                .checked_add(posting.amount.get())
                .ok_or(LedgerError::Overflow)?;
            let old = next_balances
                .get(&posting.account)
                .copied()
                .unwrap_or(MoneyMinor::new(0));
            let next = old
                .get()
                .checked_add(posting.amount.get())
                .map(MoneyMinor::new)
                .ok_or(LedgerError::Overflow)?;
            next_balances.insert(posting.account, next);
        }
        if sum != 0 {
            return Err(LedgerError::Unbalanced);
        }
        let next_id = self
            .next_transaction_id
            .checked_add(1)
            .ok_or(LedgerError::Overflow)?;
        let id = self.next_transaction_id;

        self.balances = next_balances;
        self.transactions.push(LedgerTransaction {
            transaction_id: id,
            event_seq: transaction.event_seq,
            kind: transaction.kind,
            postings: transaction.postings,
        });
        self.next_transaction_id = next_id;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn realized(amount: i64) -> NewTransaction {
        NewTransaction {
            event_seq: 7,
            kind: "REALIZED_PNL".into(),
            postings: vec![
                Posting {
                    account: LedgerAccount::Cash,
                    amount: MoneyMinor::new(amount),
                },
                Posting {
                    account: LedgerAccount::RealizedPnl,
                    amount: MoneyMinor::new(-amount),
                },
            ],
        }
    }

    #[test]
    fn balanced_transaction_updates_both_accounts() {
        let mut ledger = Ledger::new();
        assert_eq!(ledger.record(realized(25)), Ok(0));
        assert_eq!(ledger.balance(LedgerAccount::Cash), MoneyMinor::new(25));
        assert_eq!(
            ledger.balance(LedgerAccount::RealizedPnl),
            MoneyMinor::new(-25)
        );
        assert_eq!(ledger.transactions().count(), 1);
    }

    #[test]
    fn unbalanced_transaction_is_atomic() {
        let mut ledger = Ledger::new();
        ledger.record(realized(10)).unwrap();
        let before = ledger.clone();
        let bad = NewTransaction {
            event_seq: 8,
            kind: "BAD".into(),
            postings: vec![
                Posting {
                    account: LedgerAccount::Cash,
                    amount: MoneyMinor::new(5),
                },
                Posting {
                    account: LedgerAccount::Fees,
                    amount: MoneyMinor::new(-4),
                },
            ],
        };
        assert_eq!(ledger.record(bad), Err(LedgerError::Unbalanced));
        assert_eq!(ledger, before);
    }

    #[test]
    fn balance_overflow_is_atomic() {
        let mut ledger = Ledger::from_snapshot(LedgerSnapshot {
            next_transaction_id: 4,
            balances: vec![
                (LedgerAccount::Cash, MoneyMinor::new(i64::MAX)),
                (LedgerAccount::RealizedPnl, MoneyMinor::new(-i64::MAX)),
            ],
        })
        .unwrap();
        let before = ledger.clone();
        assert_eq!(ledger.record(realized(1)), Err(LedgerError::Overflow));
        assert_eq!(ledger, before);
    }

    #[test]
    fn snapshot_round_trip_preserves_economic_balances() {
        let mut ledger = Ledger::new();
        ledger.record(realized(15)).unwrap();
        let snapshot = ledger.snapshot();
        let restored = Ledger::from_snapshot(snapshot.clone()).unwrap();
        assert_eq!(restored.snapshot(), snapshot);
        assert_eq!(restored.transactions().count(), 0);
    }

    #[test]
    fn invalid_snapshot_fails_closed() {
        let unbalanced = LedgerSnapshot {
            next_transaction_id: 0,
            balances: vec![(LedgerAccount::Cash, MoneyMinor::new(1))],
        };
        assert_eq!(
            Ledger::from_snapshot(unbalanced),
            Err(LedgerError::InvalidSnapshot)
        );
    }
}
