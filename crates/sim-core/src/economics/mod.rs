//! Deterministic fees, scheduled cash flows, and corporate-action adjustments.

use std::collections::BTreeMap;

use crate::ledger::{Ledger, LedgerAccount, LedgerError, NewTransaction, Posting};
use crate::numeric::{
    DecimalScale, MoneyMinor, NumericError, PriceAtoms, QtyAtoms, RatePpb, Rounding,
    linear_notional_minor,
};
use crate::orders::{Order, OrderKind};
use crate::positions::{Position, PositionError};

const PPB_DENOMINATOR: i128 = 1_000_000_000;

/// Contract math required to convert a fill into settlement minor units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EconomicsMath {
    /// Integer contract multiplier.
    pub contract_multiplier_atoms: u64,
    /// Quantity decimal scale.
    pub qty_scale: DecimalScale,
    /// Price decimal scale.
    pub price_scale: DecimalScale,
    /// Contract multiplier decimal scale.
    pub multiplier_scale: DecimalScale,
    /// Settlement-currency decimal scale.
    pub settlement_scale: DecimalScale,
    /// Explicit notional/rate rounding policy.
    pub rounding: Rounding,
}

/// Stable fee classification used in ledger transaction kinds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiquidityRole {
    /// Resting-liquidity approximation or venue-confirmed maker fill.
    Maker,
    /// Liquidity-taking fill.
    Taker,
}

/// One exact execution-fee calculation request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionFeeInput {
    /// Logical event sequence causing the fee.
    pub event_seq: u64,
    /// Filled quantity.
    pub quantity: QtyAtoms,
    /// Execution price.
    pub price: PriceAtoms,
    /// Signed PPB rate; negative rates are rebates.
    pub rate: RatePpb,
    /// Maker/taker classification.
    pub role: LiquidityRole,
    /// Exact fixed-point conversion configuration.
    pub math: EconomicsMath,
}

/// Stable source identity for a scheduled economic event.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ScheduledEconomicId {
    /// Canonical source/provider identity.
    pub source: String,
    /// Stable upstream event identity within that source.
    pub event_id: String,
}

impl ScheduledEconomicId {
    /// Creates a non-empty deterministic source/event identity.
    ///
    /// # Errors
    /// Returns [`EconomicsError::InvalidIdentity`] for empty fields.
    pub fn new(
        source: impl Into<String>,
        event_id: impl Into<String>,
    ) -> Result<Self, EconomicsError> {
        let value = Self {
            source: source.into(),
            event_id: event_id.into(),
        };
        if value.source.is_empty() || value.event_id.is_empty() {
            return Err(EconomicsError::InvalidIdentity);
        }
        Ok(value)
    }
}

/// One signed scheduled cash flow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledCashFlow {
    /// Stable provider/source event identity.
    pub id: ScheduledEconomicId,
    /// Logical event sequence causing the posting.
    pub event_seq: u64,
    /// Signed cash delta; positive means cash received.
    pub cash_delta: MoneyMinor,
}

/// One non-negative scheduled charge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledCharge {
    /// Stable provider/source event identity.
    pub id: ScheduledEconomicId,
    /// Logical event sequence causing the posting.
    pub event_seq: u64,
    /// Positive/zero cash charge.
    pub charge: MoneyMinor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScheduledFingerprint {
    event_seq: u64,
    kind: &'static str,
    account: LedgerAccount,
    cash_delta: MoneyMinor,
}

/// Exact split ratio `numerator / denominator` applied to quantities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SplitRatio {
    /// New quantity units.
    pub numerator: u64,
    /// Old quantity units.
    pub denominator: u64,
}

impl SplitRatio {
    /// Creates a strictly positive split ratio.
    ///
    /// # Errors
    /// Returns [`EconomicsError::InvalidSplit`] when either side is zero.
    pub fn new(numerator: u64, denominator: u64) -> Result<Self, EconomicsError> {
        if numerator == 0 || denominator == 0 {
            return Err(EconomicsError::InvalidSplit);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }
}

/// Checked order values after a stock/contract split.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SplitOrderAdjustment {
    /// Adjusted total quantity.
    pub quantity: QtyAtoms,
    /// Adjusted cumulative filled quantity.
    pub filled: QtyAtoms,
    /// Adjusted order kind and all embedded prices.
    pub kind: OrderKind,
}

/// Stable economics failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EconomicsError {
    /// A source/event identity was empty.
    InvalidIdentity,
    /// A charge that must be non-negative was negative.
    NegativeCharge,
    /// An event ID was retried with changed authoritative economics.
    ScheduledConflict,
    /// A split ratio was zero or produced a non-integral exact transform.
    InvalidSplit,
    /// Checked integer arithmetic failed.
    Numeric(NumericError),
    /// Ledger rejected a transaction.
    Ledger(LedgerError),
    /// Position state was incompatible with an exact split.
    Position(PositionError),
}

impl core::fmt::Display for EconomicsError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidIdentity => formatter.write_str("economic event identity cannot be empty"),
            Self::NegativeCharge => formatter.write_str("economic charge cannot be negative"),
            Self::ScheduledConflict => {
                formatter.write_str("economic event id reused with changed posting")
            }
            Self::InvalidSplit => formatter.write_str("split ratio cannot be represented exactly"),
            Self::Numeric(error) => write!(formatter, "economics arithmetic failed: {error}"),
            Self::Ledger(error) => write!(formatter, "economics ledger posting failed: {error}"),
            Self::Position(error) => write!(formatter, "economics position adjustment failed: {error}"),
        }
    }
}

impl std::error::Error for EconomicsError {}

impl From<NumericError> for EconomicsError {
    fn from(value: NumericError) -> Self {
        Self::Numeric(value)
    }
}

impl From<LedgerError> for EconomicsError {
    fn from(value: LedgerError) -> Self {
        Self::Ledger(value)
    }
}

impl From<PositionError> for EconomicsError {
    fn from(value: PositionError) -> Self {
        Self::Position(value)
    }
}

/// Idempotent deterministic economic-event processor.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EconomicsState {
    posted_scheduled: BTreeMap<ScheduledEconomicId, ScheduledFingerprint>,
}

impl EconomicsState {
    /// Creates empty scheduled-event state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            posted_scheduled: BTreeMap::new(),
        }
    }

    /// Returns whether a scheduled source event has already posted.
    #[must_use]
    pub fn has_posted(&self, id: &ScheduledEconomicId) -> bool {
        self.posted_scheduled.contains_key(id)
    }

    /// Posts a maker/taker fee or rebate for one fill.
    ///
    /// Positive rates are charges and reduce cash; negative rates are rebates and increase
    /// cash. The signed fee is computed from exact absolute notional and a PPB rate.
    ///
    /// # Errors
    /// Returns an arithmetic/ledger error without mutating the ledger on failure.
    pub fn post_execution_fee(
        &self,
        ledger: &mut Ledger,
        input: ExecutionFeeInput,
    ) -> Result<MoneyMinor, EconomicsError> {
        let notional = linear_notional_minor(
            input.quantity,
            input.price,
            input.math.contract_multiplier_atoms,
            input.math.qty_scale,
            input.math.price_scale,
            input.math.multiplier_scale,
            input.math.settlement_scale,
            input.math.rounding,
        )?;
        let absolute_notional = notional
            .get()
            .checked_abs()
            .ok_or(NumericError::Overflow)?;
        let fee = apply_rate_ppb(
            MoneyMinor::new(absolute_notional),
            input.rate,
            input.math.rounding,
        )?;
        let kind = match input.role {
            LiquidityRole::Maker => "MAKER_FEE",
            LiquidityRole::Taker => "TAKER_FEE",
        };
        post_charge(ledger, input.event_seq, kind, LedgerAccount::Fees, fee)?;
        Ok(fee)
    }

    /// Posts one signed funding payment exactly once.
    ///
    /// Positive cash means funding received; negative means funding paid.
    ///
    /// # Errors
    /// Returns a ledger/idempotency error without consuming the event identity on failure.
    pub fn post_funding(
        &mut self,
        ledger: &mut Ledger,
        flow: ScheduledCashFlow,
    ) -> Result<bool, EconomicsError> {
        self.post_scheduled_cash_flow(
            ledger,
            flow,
            "FUNDING",
            LedgerAccount::Funding,
        )
    }

    /// Posts one non-negative borrow charge exactly once.
    ///
    /// # Errors
    /// Returns [`EconomicsError::NegativeCharge`] for a negative charge.
    pub fn post_borrow_charge(
        &mut self,
        ledger: &mut Ledger,
        charge: ScheduledCharge,
    ) -> Result<bool, EconomicsError> {
        if charge.charge.get() < 0 {
            return Err(EconomicsError::NegativeCharge);
        }
        let cash_delta = charge
            .charge
            .get()
            .checked_neg()
            .map(MoneyMinor::new)
            .ok_or(NumericError::Overflow)?;
        self.post_scheduled_cash_flow(
            ledger,
            ScheduledCashFlow {
                id: charge.id,
                event_seq: charge.event_seq,
                cash_delta,
            },
            "BORROW",
            LedgerAccount::Borrow,
        )
    }

    /// Posts one signed dividend/distribution exactly once.
    ///
    /// Positive values are cash received; negative values model a short dividend liability.
    ///
    /// # Errors
    /// Returns a ledger/idempotency error without consuming the event identity on failure.
    pub fn post_dividend(
        &mut self,
        ledger: &mut Ledger,
        flow: ScheduledCashFlow,
    ) -> Result<bool, EconomicsError> {
        self.post_scheduled_cash_flow(
            ledger,
            flow,
            "DIVIDEND",
            LedgerAccount::Dividends,
        )
    }

    /// Posts one signed futures/expiry settlement adjustment exactly once.
    ///
    /// # Errors
    /// Returns a ledger/idempotency error without consuming the event identity on failure.
    pub fn post_settlement(
        &mut self,
        ledger: &mut Ledger,
        flow: ScheduledCashFlow,
    ) -> Result<bool, EconomicsError> {
        self.post_scheduled_cash_flow(
            ledger,
            flow,
            "SETTLEMENT",
            LedgerAccount::Settlement,
        )
    }

    fn post_scheduled_cash_flow(
        &mut self,
        ledger: &mut Ledger,
        flow: ScheduledCashFlow,
        kind: &'static str,
        account: LedgerAccount,
    ) -> Result<bool, EconomicsError> {
        let fingerprint = ScheduledFingerprint {
            event_seq: flow.event_seq,
            kind,
            account,
            cash_delta: flow.cash_delta,
        };
        if let Some(existing) = self.posted_scheduled.get(&flow.id) {
            if existing == &fingerprint {
                return Ok(false);
            }
            return Err(EconomicsError::ScheduledConflict);
        }
        post_cash_flow(
            ledger,
            flow.event_seq,
            kind,
            account,
            flow.cash_delta,
        )?;
        self.posted_scheduled.insert(flow.id, fingerprint);
        Ok(true)
    }
}

/// Applies a PPB rate to settlement minor units with explicit signed rounding.
///
/// # Errors
/// Returns [`EconomicsError::Numeric`] if the multiplication/result overflows.
pub fn apply_rate_ppb(
    amount: MoneyMinor,
    rate: RatePpb,
    rounding: Rounding,
) -> Result<MoneyMinor, EconomicsError> {
    let numerator = i128::from(amount.get())
        .checked_mul(i128::from(rate.get()))
        .ok_or(NumericError::Overflow)?;
    let rounded = div_round(numerator, PPB_DENOMINATOR, rounding)?;
    let result = i64::try_from(rounded).map_err(|_| NumericError::Overflow)?;
    Ok(MoneyMinor::new(result))
}

/// Computes an exact split-adjusted position without mutating the input.
///
/// Quantity is multiplied by `numerator/denominator`; entry price uses the inverse ratio,
/// preserving pre/post basis value. Non-integral atom transforms fail closed.
///
/// # Errors
/// Returns [`EconomicsError::InvalidSplit`] or checked-overflow errors.
pub fn split_position(position: Position, ratio: SplitRatio) -> Result<Position, EconomicsError> {
    if position.quantity_atoms == 0 {
        return Position::from_snapshot(position).map_err(Into::into);
    }
    let price = position
        .average_entry_price
        .ok_or(PositionError::InvalidState)?;
    let quantity = scale_signed_exact(position.quantity_atoms, ratio.numerator, ratio.denominator)?;
    let adjusted_price = scale_price_exact(price, ratio.denominator, ratio.numerator)?;
    Position::from_snapshot(Position {
        quantity_atoms: quantity,
        average_entry_price: Some(adjusted_price),
        realized_pnl: position.realized_pnl,
    })
    .map_err(Into::into)
}

/// Computes exact split-adjusted order quantity/fills/prices.
///
/// This pure transform can be applied atomically by the simulator facade together with the
/// position/corporate-action event. No hidden rounding enters the order state machine.
///
/// # Errors
/// Returns [`EconomicsError::InvalidSplit`] for non-integral atom transforms.
pub fn split_order(order: &Order, ratio: SplitRatio) -> Result<SplitOrderAdjustment, EconomicsError> {
    let quantity = scale_unsigned_exact(order.quantity.get(), ratio.numerator, ratio.denominator)?;
    let filled = scale_unsigned_exact(order.filled.get(), ratio.numerator, ratio.denominator)?;
    let kind = match order.kind {
        OrderKind::Market => OrderKind::Market,
        OrderKind::Limit { limit_price } => OrderKind::Limit {
            limit_price: scale_price_exact(limit_price, ratio.denominator, ratio.numerator)?,
        },
        OrderKind::StopMarket { stop_price } => OrderKind::StopMarket {
            stop_price: scale_price_exact(stop_price, ratio.denominator, ratio.numerator)?,
        },
        OrderKind::StopLimit {
            stop_price,
            limit_price,
        } => OrderKind::StopLimit {
            stop_price: scale_price_exact(stop_price, ratio.denominator, ratio.numerator)?,
            limit_price: scale_price_exact(limit_price, ratio.denominator, ratio.numerator)?,
        },
    };
    Ok(SplitOrderAdjustment {
        quantity: QtyAtoms::new(quantity),
        filled: QtyAtoms::new(filled),
        kind,
    })
}

fn post_charge(
    ledger: &mut Ledger,
    event_seq: u64,
    kind: &str,
    account: LedgerAccount,
    charge: MoneyMinor,
) -> Result<(), EconomicsError> {
    let cash_delta = charge
        .get()
        .checked_neg()
        .map(MoneyMinor::new)
        .ok_or(NumericError::Overflow)?;
    post_cash_flow(ledger, event_seq, kind, account, cash_delta)
}

fn post_cash_flow(
    ledger: &mut Ledger,
    event_seq: u64,
    kind: &str,
    account: LedgerAccount,
    cash_delta: MoneyMinor,
) -> Result<(), EconomicsError> {
    let offset = cash_delta
        .get()
        .checked_neg()
        .map(MoneyMinor::new)
        .ok_or(NumericError::Overflow)?;
    ledger.record(NewTransaction {
        event_seq,
        kind: kind.into(),
        postings: vec![
            Posting {
                account: LedgerAccount::Cash,
                amount: cash_delta,
            },
            Posting {
                account,
                amount: offset,
            },
        ],
    })?;
    Ok(())
}

fn scale_signed_exact(
    value: i64,
    numerator: u64,
    denominator: u64,
) -> Result<i64, EconomicsError> {
    let product = i128::from(value)
        .checked_mul(i128::from(numerator))
        .ok_or(NumericError::Overflow)?;
    let divisor = i128::from(denominator);
    if product % divisor != 0 {
        return Err(EconomicsError::InvalidSplit);
    }
    i64::try_from(product / divisor).map_err(|_| EconomicsError::Numeric(NumericError::Overflow))
}

fn scale_unsigned_exact(
    value: u64,
    numerator: u64,
    denominator: u64,
) -> Result<u64, EconomicsError> {
    let product = u128::from(value)
        .checked_mul(u128::from(numerator))
        .ok_or(NumericError::Overflow)?;
    let divisor = u128::from(denominator);
    if product % divisor != 0 {
        return Err(EconomicsError::InvalidSplit);
    }
    u64::try_from(product / divisor).map_err(|_| EconomicsError::Numeric(NumericError::Overflow))
}

fn scale_price_exact(
    price: PriceAtoms,
    numerator: u64,
    denominator: u64,
) -> Result<PriceAtoms, EconomicsError> {
    if price.get() <= 0 {
        return Err(EconomicsError::InvalidSplit);
    }
    let value = scale_signed_exact(price.get(), numerator, denominator)?;
    if value <= 0 {
        return Err(EconomicsError::InvalidSplit);
    }
    Ok(PriceAtoms::new(value))
}

fn div_round(
    numerator: i128,
    denominator: i128,
    rounding: Rounding,
) -> Result<i128, NumericError> {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder == 0 {
        return Ok(quotient);
    }
    let sign = if numerator.is_negative() { -1 } else { 1 };
    let rounded = match rounding {
        Rounding::AwayFromZero => quotient.checked_add(sign),
        Rounding::Floor if numerator.is_negative() => quotient.checked_sub(1),
        Rounding::Ceiling if numerator.is_positive() => quotient.checked_add(1),
        Rounding::TowardZero | Rounding::Floor | Rounding::Ceiling => Some(quotient),
        Rounding::NearestTiesAway => {
            let doubled = remainder
                .abs()
                .checked_mul(2)
                .ok_or(NumericError::Overflow)?;
            if doubled >= denominator {
                quotient.checked_add(sign)
            } else {
                Some(quotient)
            }
        }
    };
    rounded.ok_or(NumericError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::{NewOrder, OrderState, Side, TimeInForce};

    fn math(rounding: Rounding) -> EconomicsMath {
        EconomicsMath {
            contract_multiplier_atoms: 1,
            qty_scale: DecimalScale::new(0).unwrap(),
            price_scale: DecimalScale::new(0).unwrap(),
            multiplier_scale: DecimalScale::new(0).unwrap(),
            settlement_scale: DecimalScale::new(0).unwrap(),
            rounding,
        }
    }

    fn scheduled(id: &str) -> ScheduledEconomicId {
        ScheduledEconomicId::new("fixture", id).unwrap()
    }

    fn flow(id: &str, event_seq: u64, cash_delta: i64) -> ScheduledCashFlow {
        ScheduledCashFlow {
            id: scheduled(id),
            event_seq,
            cash_delta: MoneyMinor::new(cash_delta),
        }
    }

    #[test]
    fn maker_fee_and_rebate_post_exact_balanced_cash() {
        let state = EconomicsState::new();
        let mut ledger = Ledger::new();
        let fee = state
            .post_execution_fee(
                &mut ledger,
                ExecutionFeeInput {
                    event_seq: 1,
                    quantity: QtyAtoms::new(10),
                    price: PriceAtoms::new(20),
                    rate: RatePpb::new(5_000_000),
                    role: LiquidityRole::Maker,
                    math: math(Rounding::TowardZero),
                },
            )
            .unwrap();
        assert_eq!(fee, MoneyMinor::new(1));
        assert_eq!(ledger.balance(LedgerAccount::Cash), MoneyMinor::new(-1));
        assert_eq!(ledger.balance(LedgerAccount::Fees), MoneyMinor::new(1));

        let rebate = state
            .post_execution_fee(
                &mut ledger,
                ExecutionFeeInput {
                    event_seq: 2,
                    quantity: QtyAtoms::new(10),
                    price: PriceAtoms::new(20),
                    rate: RatePpb::new(-5_000_000),
                    role: LiquidityRole::Maker,
                    math: math(Rounding::TowardZero),
                },
            )
            .unwrap();
        assert_eq!(rebate, MoneyMinor::new(-1));
        assert_eq!(ledger.balance(LedgerAccount::Cash), MoneyMinor::new(0));
        assert_eq!(ledger.balance(LedgerAccount::Fees), MoneyMinor::new(0));
    }

    #[test]
    fn ppb_rounding_is_signed_and_explicit() {
        let amount = MoneyMinor::new(1);
        let rate = RatePpb::new(500_000_000);
        assert_eq!(
            apply_rate_ppb(amount, rate, Rounding::TowardZero).unwrap(),
            MoneyMinor::new(0)
        );
        assert_eq!(
            apply_rate_ppb(amount, rate, Rounding::NearestTiesAway).unwrap(),
            MoneyMinor::new(1)
        );
        assert_eq!(
            apply_rate_ppb(
                amount,
                RatePpb::new(-500_000_000),
                Rounding::Floor,
            )
            .unwrap(),
            MoneyMinor::new(-1)
        );
    }

    #[test]
    fn scheduled_payments_are_idempotent_and_conflict_safe() {
        let mut state = EconomicsState::new();
        let mut ledger = Ledger::new();
        assert!(state.post_funding(&mut ledger, flow("funding-1", 5, -7)).unwrap());
        assert!(!state.post_funding(&mut ledger, flow("funding-1", 5, -7)).unwrap());
        assert_eq!(
            state.post_funding(&mut ledger, flow("funding-1", 5, -8)),
            Err(EconomicsError::ScheduledConflict)
        );
        assert_eq!(ledger.transactions().count(), 1);
        assert_eq!(ledger.balance(LedgerAccount::Cash), MoneyMinor::new(-7));
        assert_eq!(ledger.balance(LedgerAccount::Funding), MoneyMinor::new(7));
    }

    #[test]
    fn failed_scheduled_post_does_not_consume_identity() {
        let mut state = EconomicsState::new();
        let mut ledger = Ledger::from_snapshot(crate::ledger::LedgerSnapshot {
            next_transaction_id: 0,
            balances: vec![
                (LedgerAccount::Cash, MoneyMinor::new(i64::MAX)),
                (LedgerAccount::Funding, MoneyMinor::new(-i64::MAX)),
            ],
        })
        .unwrap();
        let id = scheduled("overflow");
        assert!(state
            .post_funding(
                &mut ledger,
                ScheduledCashFlow {
                    id: id.clone(),
                    event_seq: 1,
                    cash_delta: MoneyMinor::new(1),
                },
            )
            .is_err());
        assert!(!state.has_posted(&id));
    }

    #[test]
    fn borrow_dividend_and_settlement_use_distinct_balanced_accounts() {
        let mut state = EconomicsState::new();
        let mut ledger = Ledger::new();
        state
            .post_borrow_charge(
                &mut ledger,
                ScheduledCharge {
                    id: scheduled("borrow"),
                    event_seq: 1,
                    charge: MoneyMinor::new(3),
                },
            )
            .unwrap();
        state
            .post_dividend(&mut ledger, flow("dividend", 2, 8))
            .unwrap();
        state
            .post_settlement(&mut ledger, flow("settlement", 3, -2))
            .unwrap();
        assert_eq!(ledger.balance(LedgerAccount::Cash), MoneyMinor::new(3));
        assert_eq!(ledger.balance(LedgerAccount::Borrow), MoneyMinor::new(3));
        assert_eq!(ledger.balance(LedgerAccount::Dividends), MoneyMinor::new(-8));
        assert_eq!(ledger.balance(LedgerAccount::Settlement), MoneyMinor::new(2));
    }

    #[test]
    fn forward_split_preserves_position_basis_value_exactly() {
        let ratio = SplitRatio::new(2, 1).unwrap();
        let position = Position {
            quantity_atoms: 10,
            average_entry_price: Some(PriceAtoms::new(100)),
            realized_pnl: MoneyMinor::new(7),
        };
        let adjusted = split_position(position, ratio).unwrap();
        assert_eq!(adjusted.quantity_atoms, 20);
        assert_eq!(adjusted.average_entry_price, Some(PriceAtoms::new(50)));
        assert_eq!(adjusted.realized_pnl, MoneyMinor::new(7));
    }

    #[test]
    fn split_adjusts_working_order_quantity_fills_and_prices() {
        let mut orders = OrderState::new();
        let submitted = orders
            .submit(
                NewOrder {
                    client_order_id: "split-order".into(),
                    instrument_id: "SYNTH".into(),
                    side: Side::Buy,
                    quantity: QtyAtoms::new(10),
                    kind: OrderKind::Limit {
                        limit_price: PriceAtoms::new(100),
                    },
                    time_in_force: TimeInForce::Gtc,
                    reduce_only: false,
                    post_only: false,
                    marketable_only: false,
                    submitted_at_event_seq: 1,
                },
                0,
                None,
            )
            .unwrap();
        orders
            .record_fill(submitted.order_id, QtyAtoms::new(2))
            .unwrap();
        let order = orders.get(submitted.order_id).unwrap();
        let adjusted = split_order(order, SplitRatio::new(2, 1).unwrap()).unwrap();
        assert_eq!(adjusted.quantity, QtyAtoms::new(20));
        assert_eq!(adjusted.filled, QtyAtoms::new(4));
        assert_eq!(
            adjusted.kind,
            OrderKind::Limit {
                limit_price: PriceAtoms::new(50),
            }
        );
    }

    #[test]
    fn inexact_split_fails_closed() {
        let position = Position {
            quantity_atoms: 1,
            average_entry_price: Some(PriceAtoms::new(100)),
            realized_pnl: MoneyMinor::new(0),
        };
        assert_eq!(
            split_position(position, SplitRatio::new(3, 2).unwrap()),
            Err(EconomicsError::InvalidSplit)
        );
    }
}