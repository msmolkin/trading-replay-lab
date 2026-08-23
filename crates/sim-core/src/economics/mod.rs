//! Deterministic fees, scheduled cash flows, and corporate-action adjustments.

use std::collections::BTreeSet;

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
    pub fn new(source: impl Into<String>, event_id: impl Into<String>) -> Result<Self, EconomicsError> {
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
    posted_scheduled: BTreeSet<ScheduledEconomicId>,
}

impl EconomicsState {
    /// Creates empty scheduled-event state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            posted_scheduled: BTreeSet::new(),
        }
    }

    /// Returns whether a scheduled source event has already posted.
    #[must_use]
    pub fn has_posted(&self, id: &ScheduledEconomicId) -> bool {
        self.posted_scheduled.contains(id)
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
        *,
        event_seq: u64,
        quantity: QtyAtoms,
        price: PriceAtoms,
        rate: RatePpb,
        role: LiquidityRole,
        math: EconomicsMath,
    ) -> Result<MoneyMinor, EconomicsError> {
        let notional = linear_notional_minor(
            quantity,
            price,
            math.contract_multiplier_atoms,
            math.qty_scale,
            math.price_scale,
            math.multiplier_scale,
            math.settlement_scale,
            math.rounding,
        )?;
        let absolute_notional = notional
            .get()
            .checked_abs()
            .ok_or(NumericError::Overflow)?;
        let fee = apply_rate_ppb(MoneyMinor::new(absolute_notional), rate, math.rounding)?;
        let kind = match role {
            LiquidityRole::Maker => "MAKER_FEE",
            LiquidityRole::Taker => "TAKER_FEE",
        };
        post_charge(ledger, event_seq, kind, LedgerAccount::Fees, fee)?;
        Ok(fee)
    }

    /// Posts one signed funding payment exactly once.
    ///
    /// Positive `cash_delta` means the account receives funding; negative means it pays.
    /// Exact retries of the same scheduled id are no-ops and return `false`.
    ///
    /// # Errors
    /// Returns a ledger error without recording the scheduled identity on failure.
    pub fn post_funding(
        &mut self,
        ledger: &mut Ledger,
        *,
        id: ScheduledEconomicId,
        event_seq: u64,
        cash_delta: MoneyMinor,
    ) -> Result<bool, EconomicsError> {
        self.post_scheduled_cash_flow(
            ledger,
            id,
            event_seq,
            "FUNDING",
            LedgerAccount::Funding,
            cash_delta,
        )
    }

    /// Posts one non-negative borrow charge exactly once.
    ///
    /// # Errors
    /// Returns [`EconomicsError::NegativeCharge`] for a negative charge.
    pub fn post_borrow_charge(
        &mut self,
        ledger: &mut Ledger,
        *,
        id: ScheduledEconomicId,
        event_seq: u64,
        charge: MoneyMinor,
    ) -> Result<bool, EconomicsError> {
        if charge.get() < 0 {
            return Err(EconomicsError::NegativeCharge);
        }
        let cash_delta = charge
            .get()
            .checked_neg()
            .map(MoneyMinor::new)
            .ok_or(NumericError::Overflow)?;
        self.post_scheduled_cash_flow(
            ledger,
            id,
            event_seq,
            "BORROW",
            LedgerAccount::Borrow,
            cash_delta,
        )
    }

    /// Posts one signed dividend/distribution exactly once.
    ///
    /// Positive values are cash received; negative values model a short dividend liability.
    ///
    /// # Errors
    /// Returns a ledger error without recording the scheduled identity on failure.
    pub fn post_dividend(
        &mut self,
        ledger: &mut Ledger,
        *,
        id: ScheduledEconomicId,
        event_seq: u64,
        cash_delta: MoneyMinor,
    ) -> Result<bool, EconomicsError> {
        self.post_scheduled_cash_flow(
            ledger,
            id,
            event_seq,
            "DIVIDEND",
            LedgerAccount::Dividends,
            cash_delta,
        )
    }

    /// Posts one signed futures/expiry settlement adjustment exactly once.
    ///
    /// # Errors
    /// Returns a ledger error without recording the scheduled identity on failure.
    pub fn post_settlement(
        &mut self,
        ledger: &mut Ledger,
        *,
        id: ScheduledEconomicId,
        event_seq: u64,
        cash_delta: MoneyMinor,
    ) -> Result<bool, EconomicsError> {
        self.post_scheduled_cash_flow(
            ledger,
            id,
            event_seq,
            "SETTLEMENT",
            LedgerAccount::Settlement,
            cash_delta,
        )
    }

    fn post_scheduled_cash_flow(
        &mut self,
        ledger: &mut Ledger,
        id: ScheduledEconomicId,
        event_seq: u64,
        kind: &str,
        account: LedgerAccount,
        cash_delta: MoneyMinor,
    ) -> Result<bool, EconomicsError> {
        if self.posted_scheduled.contains(&id) {
            return Ok(false);
        }
        post_cash_flow(ledger, event_seq, kind, account, cash_delta)?;
        self.posted_scheduled.insert(id);
        Ok(true)
    }
}

/// Applies a PPB rate to settlement minor units with an explicit signed rounding policy.
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
/// Quantity is multiplied by `numerator/denominator`; entry price is transformed by the
/// inverse ratio so pre/post basis value is preserved. Non-integral atom transforms fail
/// closed rather than silently round.
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
/// This pure transform is deliberately separate from the order state machine. The facade
/// can apply the returned values atomically together with the position/corporate-action
/// event, while this module owns the economic transform and exactness checks.
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

fn scale_signed_exact(value: i64, numerator: u64, denominator: u64) -> Result<i64, EconomicsError> {
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

fn div_round(numerator: i128, denominator: i128, rounding: Rounding) -> Result<i128, NumericError> {
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
    use crate::orders::{OrderId, OrderStatus, Side, TimeInForce};

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

    #[test]
    fn maker_fee_and_rebate_post_exact_balanced_cash() {
        let state = EconomicsState::new();
        let mut ledger = Ledger::new();
        let fee = state
            .post_execution_fee(
                &mut ledger,
                event_seq: 1,
                quantity: QtyAtoms::new(10),
                price: PriceAtoms::new(20),
                rate: RatePpb::new(5_000_000),
                role: LiquidityRole::Maker,
                math: math(Rounding::TowardZero),
            )
            .unwrap();
        assert_eq!(fee, MoneyMinor::new(1));
        assert_eq!(ledger.balance(LedgerAccount::Cash), MoneyMinor::new(-1));
        assert_eq!(ledger.balance(LedgerAccount::Fees), MoneyMinor::new(1));

        let rebate = state
            .post_execution_fee(
                &mut ledger,
                event_seq: 2,
                quantity: QtyAtoms::new(10),
                price: PriceAtoms::new(20),
                rate: RatePpb::new(-5_000_000),
                role: LiquidityRole::Maker,
                math: math(Rounding::TowardZero),
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
            apply_rate_ppb(amount, RatePpb::new(-500_000_000), Rounding::Floor).unwrap(),
            MoneyMinor::new(-1)
        );
    }

    #[test]
    fn scheduled_payments_post_exactly_once() {
        let mut state = EconomicsState::new();
        let mut ledger = Ledger::new();
        assert!(
            state
                .post_funding(
                    &mut ledger,
                    id: scheduled("funding-1"),
                    event_seq: 5,
                    cash_delta: MoneyMinor::new(-7),
                )
                .unwrap()
        );
        assert!(
            !state
                .post_funding(
                    &mut ledger,
                    id: scheduled("funding-1"),
                    event_seq: 5,
                    cash_delta: MoneyMinor::new(-7),
                )
                .unwrap()
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
                id: id.clone(),
                event_seq: 1,
                cash_delta: MoneyMinor::new(1),
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
                id: scheduled("borrow"),
                event_seq: 1,
                charge: MoneyMinor::new(3),
            )
            .unwrap();
        state
            .post_dividend(
                &mut ledger,
                id: scheduled("dividend"),
                event_seq: 2,
                cash_delta: MoneyMinor::new(8),
            )
            .unwrap();
        state
            .post_settlement(
                &mut ledger,
                id: scheduled("settlement"),
                event_seq: 3,
                cash_delta: MoneyMinor::new(-2),
            )
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
        let order = Order {
            id: OrderId::from_raw_for_test(9),
            client_order_id: "split-order".into(),
            instrument_id: "SYNTH".into(),
            side: Side::Buy,
            quantity: QtyAtoms::new(10),
            filled: QtyAtoms::new(2),
            kind: OrderKind::StopLimit {
                stop_price: PriceAtoms::new(110),
                limit_price: PriceAtoms::new(100),
            },
            time_in_force: TimeInForce::Gtc,
            reduce_only: false,
            post_only: false,
            marketable_only: false,
            submitted_at_event_seq: 1,
            revision: 0,
            status: OrderStatus::PartiallyFilled,
        };
        let adjusted = split_order(&order, SplitRatio::new(2, 1).unwrap()).unwrap();
        assert_eq!(adjusted.quantity, QtyAtoms::new(20));
        assert_eq!(adjusted.filled, QtyAtoms::new(4));
        assert_eq!(
            adjusted.kind,
            OrderKind::StopLimit {
                stop_price: PriceAtoms::new(55),
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
