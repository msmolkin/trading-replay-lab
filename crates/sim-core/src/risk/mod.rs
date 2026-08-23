//! Deterministic isolated-margin, leverage, pre-trade, and liquidation rules.

use crate::economics::{EconomicsError, EconomicsMath, apply_rate_ppb};
use crate::numeric::{
    MoneyMinor, NumericError, PriceAtoms, QtyAtoms, RatePpb, Rounding, linear_notional_minor,
};
use crate::orders::{OrderState, Side};
use crate::positions::Position;

const MAX_LEVERAGE: u8 = 50;
const PPB_ONE: i64 = 1_000_000_000;

/// Valid synthetic isolated leverage in the supported 1x through 50x range.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Leverage(u8);

impl Leverage {
    /// Creates a supported leverage value.
    ///
    /// # Errors
    /// Returns [`RiskError::InvalidLeverage`] outside 1..=50.
    pub fn new(value: u8) -> Result<Self, RiskError> {
        if (1..=MAX_LEVERAGE).contains(&value) {
            Ok(Self(value))
        } else {
            Err(RiskError::InvalidLeverage)
        }
    }

    /// Returns the integer leverage multiple.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Risk rules committed with a session ruleset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RiskProfile {
    /// Maintenance rate applied to absolute marked notional.
    pub maintenance_rate: RatePpb,
    /// Exact contract/scaling parameters.
    pub math: EconomicsMath,
}

impl RiskProfile {
    /// Validates a maintenance rate in the closed interval zero through one hundred percent.
    ///
    /// # Errors
    /// Returns [`RiskError::InvalidMaintenanceRate`] for negative or above-100% rates.
    pub fn validate(self) -> Result<(), RiskError> {
        if (0..=PPB_ONE).contains(&self.maintenance_rate.get()) {
            Ok(())
        } else {
            Err(RiskError::InvalidMaintenanceRate)
        }
    }
}

/// Complete marked margin view for one isolated instrument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarginSnapshot {
    /// Account equity supplied by authoritative accounting/read-model state.
    pub equity: MoneyMinor,
    /// Initial margin required by the current position.
    pub position_initial_margin: MoneyMinor,
    /// Additional initial margin reserved for non-reduce-only working orders.
    pub working_order_margin: MoneyMinor,
    /// Maintenance margin on the current position.
    pub maintenance_margin: MoneyMinor,
    /// Equity remaining after initial position and working-order reservations.
    pub available_margin: MoneyMinor,
}

impl MarginSnapshot {
    /// Total initial requirement before a new expansion.
    ///
    /// # Errors
    /// Returns [`RiskError::Numeric`] if checked margin addition overflows.
    pub fn total_initial_requirement(self) -> Result<MoneyMinor, RiskError> {
        checked_money_add(self.position_initial_margin, self.working_order_margin)
    }
}

/// Result of a pre-trade margin check.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreTradeDecision {
    /// Quantity that can reduce opposite exposure without opening new risk.
    pub close_quantity: QtyAtoms,
    /// Requested quantity that would create/increase exposure after the close leg.
    pub requested_open_quantity: QtyAtoms,
    /// Open quantity affordable under the supplied equity/reservation budget.
    pub accepted_open_quantity: QtyAtoms,
    /// Total executable quantity: close leg plus accepted open leg.
    pub accepted_quantity: QtyAtoms,
    /// Margin required if the complete requested open leg were accepted.
    pub requested_final_initial_margin: MoneyMinor,
    /// Equity left for position margin after already-reserved working-order margin.
    pub position_margin_budget: MoneyMinor,
}

impl PreTradeDecision {
    /// Returns whether the full requested quantity passes the margin check.
    #[must_use]
    pub const fn fully_accepted(self) -> bool {
        self.requested_open_quantity.get() == self.accepted_open_quantity.get()
    }
}

/// Explicit liquidation lifecycle. A trigger is observable before forced execution starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiquidationState {
    /// Equity is above maintenance or the position is flat.
    Healthy,
    /// Maintenance threshold has been breached and liquidation is required.
    Triggered,
    /// Forced close execution is in progress.
    Liquidating,
    /// Forced close completed and the isolated position must be flat.
    Liquidated,
}

/// Stable fail-closed risk errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RiskError {
    /// Leverage must be in the inclusive 1x through 50x range.
    InvalidLeverage,
    /// Maintenance rate must be between zero and one hundred percent.
    InvalidMaintenanceRate,
    /// Mark/reservation price must be positive.
    InvalidPrice,
    /// Requested leverage would leave insufficient equity for current risk.
    InsufficientMargin,
    /// Liquidation lifecycle transition is invalid.
    InvalidLiquidationTransition,
    /// Checked fixed-point arithmetic failed.
    Numeric(NumericError),
    /// Shared exact rate arithmetic failed.
    Economics(EconomicsError),
}

impl core::fmt::Display for RiskError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidLeverage => formatter.write_str("leverage must be between 1x and 50x"),
            Self::InvalidMaintenanceRate => {
                formatter.write_str("maintenance rate must be between 0 and 1_000_000_000 ppb")
            }
            Self::InvalidPrice => formatter.write_str("risk mark price must be positive"),
            Self::InsufficientMargin => {
                formatter.write_str("insufficient margin for requested risk")
            }
            Self::InvalidLiquidationTransition => {
                formatter.write_str("invalid liquidation state transition")
            }
            Self::Numeric(error) => write!(formatter, "risk arithmetic failed: {error}"),
            Self::Economics(error) => write!(formatter, "risk rate arithmetic failed: {error}"),
        }
    }
}

impl std::error::Error for RiskError {}

impl From<NumericError> for RiskError {
    fn from(value: NumericError) -> Self {
        Self::Numeric(value)
    }
}

impl From<EconomicsError> for RiskError {
    fn from(value: EconomicsError) -> Self {
        Self::Economics(value)
    }
}

/// Mutable leverage and liquidation state. Economic position/P&L remains owned elsewhere.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RiskState {
    leverage: Leverage,
    liquidation: LiquidationState,
}

impl RiskState {
    /// Creates healthy risk state at a validated leverage.
    #[must_use]
    pub const fn new(leverage: Leverage) -> Self {
        Self {
            leverage,
            liquidation: LiquidationState::Healthy,
        }
    }

    /// Returns the current leverage multiple.
    #[must_use]
    pub const fn leverage(self) -> Leverage {
        self.leverage
    }

    /// Returns the current liquidation lifecycle state.
    #[must_use]
    pub const fn liquidation_state(self) -> LiquidationState {
        self.liquidation
    }

    /// Changes leverage atomically after recomputing current position/order requirements.
    ///
    /// Increasing leverage lowers initial requirements but never changes quantity, basis,
    /// realized P&L, cash, or equity. A decrease that current equity cannot support fails
    /// without changing the requested/current leverage.
    ///
    /// # Errors
    /// Returns [`RiskError::InsufficientMargin`] or checked arithmetic errors without mutation.
    #[allow(clippy::too_many_arguments)]
    pub fn set_leverage(
        &mut self,
        requested: Leverage,
        equity: MoneyMinor,
        position: Position,
        instrument_id: &str,
        orders: &OrderState,
        mark_price: PriceAtoms,
        profile: RiskProfile,
    ) -> Result<MarginSnapshot, RiskError> {
        let snapshot = margin_snapshot(
            equity,
            position,
            instrument_id,
            orders,
            mark_price,
            requested,
            profile,
        )?;
        let required = snapshot.total_initial_requirement()?;
        if equity.get() < required.get() {
            return Err(RiskError::InsufficientMargin);
        }
        self.leverage = requested;
        Ok(snapshot)
    }

    /// Evaluates the maintenance threshold and records the first deterministic trigger.
    ///
    /// Returns `true` only for the transition from healthy to triggered. Flat positions do
    /// not liquidate even when account equity is non-positive.
    pub fn evaluate_liquidation(&mut self, position: Position, snapshot: MarginSnapshot) -> bool {
        if self.liquidation != LiquidationState::Healthy || position.quantity_atoms == 0 {
            return false;
        }
        if snapshot.equity.get() <= snapshot.maintenance_margin.get() {
            self.liquidation = LiquidationState::Triggered;
            return true;
        }
        false
    }

    /// Starts deterministic forced-close execution after a trigger.
    ///
    /// # Errors
    /// Returns [`RiskError::InvalidLiquidationTransition`] unless currently triggered.
    pub fn begin_liquidation(&mut self) -> Result<(), RiskError> {
        if self.liquidation != LiquidationState::Triggered {
            return Err(RiskError::InvalidLiquidationTransition);
        }
        self.liquidation = LiquidationState::Liquidating;
        Ok(())
    }

    /// Completes forced liquidation only after the authoritative position is flat.
    ///
    /// # Errors
    /// Returns [`RiskError::InvalidLiquidationTransition`] for a non-liquidating state or
    /// non-flat position.
    pub fn complete_liquidation(&mut self, position: Position) -> Result<(), RiskError> {
        if self.liquidation != LiquidationState::Liquidating || position.quantity_atoms != 0 {
            return Err(RiskError::InvalidLiquidationTransition);
        }
        self.liquidation = LiquidationState::Liquidated;
        Ok(())
    }
}

/// Computes a full marked isolated-margin snapshot.
///
/// Working-order reservation is based on the maximum absolute exposure reachable if all
/// non-reduce-only live buy orders or all non-reduce-only live sell orders execute. Existing
/// position closing capacity is credited at most once, avoiding the common two-close-order
/// under-reservation bug while remaining conservative when both sides are working.
///
/// # Errors
/// Returns validation or checked-arithmetic errors.
#[allow(clippy::too_many_arguments)]
pub fn margin_snapshot(
    equity: MoneyMinor,
    position: Position,
    instrument_id: &str,
    orders: &OrderState,
    mark_price: PriceAtoms,
    leverage: Leverage,
    profile: RiskProfile,
) -> Result<MarginSnapshot, RiskError> {
    profile.validate()?;
    validate_price(mark_price)?;

    let position_quantity = QtyAtoms::new(position.quantity_atoms.unsigned_abs());
    let position_initial_margin =
        initial_margin(position_quantity, mark_price, leverage, profile.math)?;
    let maintenance_margin = maintenance_margin(
        position_quantity,
        mark_price,
        profile.maintenance_rate,
        profile.math,
    )?;
    let expansion = working_order_expansion_atoms(orders, instrument_id, position.quantity_atoms)?;
    let working_order_margin =
        initial_margin(QtyAtoms::new(expansion), mark_price, leverage, profile.math)?;
    let total_initial = checked_money_add(position_initial_margin, working_order_margin)?;
    let available = equity
        .get()
        .checked_sub(total_initial.get())
        .map(MoneyMinor::new)
        .ok_or(NumericError::Overflow)?;

    Ok(MarginSnapshot {
        equity,
        position_initial_margin,
        working_order_margin,
        maintenance_margin,
        available_margin: available,
    })
}

/// Performs a margin precheck that always permits the risk-reducing leg of a reversal.
///
/// `reserved_working_margin` is held fixed during this command check. If a command crosses
/// through zero, the close leg is admitted first and a deterministic binary search finds the
/// largest residual open quantity affordable under the remaining equity budget.
///
/// # Errors
/// Returns validation or checked-arithmetic errors.
#[allow(clippy::too_many_arguments)]
pub fn precheck_fill(
    equity: MoneyMinor,
    position: Position,
    side: Side,
    requested_quantity: QtyAtoms,
    execution_price: PriceAtoms,
    reserved_working_margin: MoneyMinor,
    leverage: Leverage,
    profile: RiskProfile,
) -> Result<PreTradeDecision, RiskError> {
    profile.validate()?;
    validate_price(execution_price)?;
    if reserved_working_margin.get() < 0 {
        return Err(RiskError::InsufficientMargin);
    }

    let current_abs = position.quantity_atoms.unsigned_abs();
    let opposite = match side {
        Side::Buy => position.quantity_atoms < 0,
        Side::Sell => position.quantity_atoms > 0,
    };
    let close = if opposite {
        requested_quantity.get().min(current_abs)
    } else {
        0
    };
    let requested_open = requested_quantity.get() - close;

    if requested_open == 0 {
        let remaining_abs = current_abs - close;
        let final_margin = initial_margin(
            QtyAtoms::new(remaining_abs),
            execution_price,
            leverage,
            profile.math,
        )?;
        let budget = checked_money_sub(equity, reserved_working_margin)?;
        return Ok(PreTradeDecision {
            close_quantity: QtyAtoms::new(close),
            requested_open_quantity: QtyAtoms::new(0),
            accepted_open_quantity: QtyAtoms::new(0),
            accepted_quantity: requested_quantity,
            requested_final_initial_margin: final_margin,
            position_margin_budget: budget,
        });
    }

    let base_abs = if opposite { 0 } else { current_abs };
    let desired_final_abs = base_abs
        .checked_add(requested_open)
        .ok_or(NumericError::Overflow)?;
    let requested_final_margin = initial_margin(
        QtyAtoms::new(desired_final_abs),
        execution_price,
        leverage,
        profile.math,
    )?;
    let budget = checked_money_sub(equity, reserved_working_margin)?;
    let accepted_open = max_affordable_additional_quantity(
        base_abs,
        requested_open,
        budget,
        execution_price,
        leverage,
        profile.math,
    )?;
    let accepted_total = close
        .checked_add(accepted_open)
        .ok_or(NumericError::Overflow)?;

    Ok(PreTradeDecision {
        close_quantity: QtyAtoms::new(close),
        requested_open_quantity: QtyAtoms::new(requested_open),
        accepted_open_quantity: QtyAtoms::new(accepted_open),
        accepted_quantity: QtyAtoms::new(accepted_total),
        requested_final_initial_margin: requested_final_margin,
        position_margin_budget: budget,
    })
}

/// Computes initial margin using conservative ceiling division of positive marked notional.
///
/// # Errors
/// Returns checked arithmetic errors or [`RiskError::InvalidPrice`].
pub fn initial_margin(
    quantity: QtyAtoms,
    mark_price: PriceAtoms,
    leverage: Leverage,
    math: EconomicsMath,
) -> Result<MoneyMinor, RiskError> {
    let notional = absolute_notional(quantity, mark_price, math)?;
    let denominator = i128::from(leverage.get());
    let quotient = i128::from(notional) / denominator;
    let remainder = i128::from(notional) % denominator;
    let rounded = if remainder == 0 {
        quotient
    } else {
        quotient.checked_add(1).ok_or(NumericError::Overflow)?
    };
    let value = i64::try_from(rounded).map_err(|_| NumericError::Overflow)?;
    Ok(MoneyMinor::new(value))
}

/// Computes maintenance margin from absolute marked notional and a validated PPB rate.
///
/// # Errors
/// Returns checked arithmetic errors or invalid profile/price errors.
pub fn maintenance_margin(
    quantity: QtyAtoms,
    mark_price: PriceAtoms,
    maintenance_rate: RatePpb,
    math: EconomicsMath,
) -> Result<MoneyMinor, RiskError> {
    if !(0..=PPB_ONE).contains(&maintenance_rate.get()) {
        return Err(RiskError::InvalidMaintenanceRate);
    }
    let notional = absolute_notional(quantity, mark_price, math)?;
    apply_rate_ppb(
        MoneyMinor::new(notional),
        maintenance_rate,
        Rounding::Ceiling,
    )
    .map_err(Into::into)
}

fn absolute_notional(
    quantity: QtyAtoms,
    mark_price: PriceAtoms,
    math: EconomicsMath,
) -> Result<i64, RiskError> {
    validate_price(mark_price)?;
    if quantity.get() == 0 {
        return Ok(0);
    }
    let notional = linear_notional_minor(
        quantity,
        mark_price,
        math.contract_multiplier_atoms,
        math.qty_scale,
        math.price_scale,
        math.multiplier_scale,
        math.settlement_scale,
        Rounding::Ceiling,
    )?;
    notional
        .get()
        .checked_abs()
        .ok_or(NumericError::Overflow.into())
}

fn validate_price(price: PriceAtoms) -> Result<(), RiskError> {
    if price.get() <= 0 {
        Err(RiskError::InvalidPrice)
    } else {
        Ok(())
    }
}

fn working_order_expansion_atoms(
    orders: &OrderState,
    instrument_id: &str,
    position_atoms: i64,
) -> Result<u64, RiskError> {
    let mut buys = 0_u128;
    let mut sells = 0_u128;
    for order in orders.iter() {
        if order.instrument_id != instrument_id || order.status.is_terminal() || order.reduce_only {
            continue;
        }
        let remaining = u128::from(order.remaining().get());
        match order.side {
            Side::Buy => buys = buys.checked_add(remaining).ok_or(NumericError::Overflow)?,
            Side::Sell => sells = sells.checked_add(remaining).ok_or(NumericError::Overflow)?,
        }
    }

    let base = i128::from(position_atoms);
    let buys = i128::try_from(buys).map_err(|_| NumericError::Overflow)?;
    let sells = i128::try_from(sells).map_err(|_| NumericError::Overflow)?;
    let long_extreme = base.checked_add(buys).ok_or(NumericError::Overflow)?;
    let short_extreme = base.checked_sub(sells).ok_or(NumericError::Overflow)?;
    let base_abs = base.abs();
    let max_abs = base_abs.max(long_extreme.abs()).max(short_extreme.abs());
    let expansion = max_abs
        .checked_sub(base_abs)
        .ok_or(NumericError::Overflow)?;
    u64::try_from(expansion).map_err(|_| NumericError::Overflow.into())
}

fn max_affordable_additional_quantity(
    base_abs: u64,
    requested_additional: u64,
    budget: MoneyMinor,
    price: PriceAtoms,
    leverage: Leverage,
    math: EconomicsMath,
) -> Result<u64, RiskError> {
    if requested_additional == 0 || budget.get() < 0 {
        return Ok(0);
    }
    let base_margin = initial_margin(QtyAtoms::new(base_abs), price, leverage, math)?;
    if base_margin.get() > budget.get() {
        return Ok(0);
    }

    let mut low = 0_u64;
    let mut high = requested_additional;
    while low < high {
        let span = high - low;
        let midpoint = low
            .checked_add(span / 2)
            .and_then(|value| value.checked_add(span % 2))
            .ok_or(NumericError::Overflow)?;
        let total = base_abs
            .checked_add(midpoint)
            .ok_or(NumericError::Overflow)?;
        let margin = initial_margin(QtyAtoms::new(total), price, leverage, math)?;
        if margin.get() <= budget.get() {
            low = midpoint;
        } else {
            high = midpoint - 1;
        }
    }
    Ok(low)
}

fn checked_money_add(left: MoneyMinor, right: MoneyMinor) -> Result<MoneyMinor, RiskError> {
    left.get()
        .checked_add(right.get())
        .map(MoneyMinor::new)
        .ok_or(NumericError::Overflow.into())
}

fn checked_money_sub(left: MoneyMinor, right: MoneyMinor) -> Result<MoneyMinor, RiskError> {
    left.get()
        .checked_sub(right.get())
        .map(MoneyMinor::new)
        .ok_or(NumericError::Overflow.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::numeric::DecimalScale;
    use crate::orders::{NewOrder, OrderKind, TimeInForce};

    fn math() -> EconomicsMath {
        EconomicsMath {
            contract_multiplier_atoms: 1,
            qty_scale: DecimalScale::new(0).unwrap(),
            price_scale: DecimalScale::new(0).unwrap(),
            multiplier_scale: DecimalScale::new(0).unwrap(),
            settlement_scale: DecimalScale::new(0).unwrap(),
            rounding: Rounding::TowardZero,
        }
    }

    fn profile() -> RiskProfile {
        RiskProfile {
            maintenance_rate: RatePpb::new(250_000_000),
            math: math(),
        }
    }

    fn leverage(value: u8) -> Leverage {
        Leverage::new(value).unwrap()
    }

    fn position(quantity_atoms: i64) -> Position {
        Position::from_snapshot(Position {
            quantity_atoms,
            average_entry_price: (quantity_atoms != 0).then_some(PriceAtoms::new(100)),
            realized_pnl: MoneyMinor::new(17),
        })
        .unwrap()
    }

    fn submit_order(
        orders: &mut OrderState,
        client_id: &str,
        side: Side,
        quantity: u64,
        reduce_only: bool,
        position_atoms: i64,
    ) {
        orders
            .submit(
                NewOrder {
                    client_order_id: client_id.into(),
                    instrument_id: "SYNTH".into(),
                    side,
                    quantity: QtyAtoms::new(quantity),
                    kind: OrderKind::Limit {
                        limit_price: PriceAtoms::new(100),
                    },
                    time_in_force: TimeInForce::Gtc,
                    reduce_only,
                    post_only: false,
                    marketable_only: false,
                    submitted_at_event_seq: 1,
                },
                position_atoms,
                None,
            )
            .unwrap();
    }

    #[test]
    fn leverage_range_is_exactly_one_through_fifty() {
        assert_eq!(Leverage::new(0), Err(RiskError::InvalidLeverage));
        assert_eq!(Leverage::new(1).unwrap().get(), 1);
        assert_eq!(Leverage::new(50).unwrap().get(), 50);
        assert_eq!(Leverage::new(51), Err(RiskError::InvalidLeverage));
    }

    #[test]
    fn leverage_changes_margin_not_position_or_pnl() {
        let mut state = RiskState::new(leverage(2));
        let position = position(10);
        let before = position;
        let orders = OrderState::new();
        let snapshot = state
            .set_leverage(
                leverage(5),
                MoneyMinor::new(10_000),
                position,
                "SYNTH",
                &orders,
                PriceAtoms::new(100),
                profile(),
            )
            .unwrap();
        assert_eq!(state.leverage(), leverage(5));
        assert_eq!(position, before);
        assert_eq!(snapshot.position_initial_margin, MoneyMinor::new(200));
        assert_eq!(position.realized_pnl, MoneyMinor::new(17));
    }

    #[test]
    fn invalid_leverage_decrease_is_atomic() {
        let mut state = RiskState::new(leverage(10));
        let orders = OrderState::new();
        let before = state;
        assert_eq!(
            state.set_leverage(
                leverage(1),
                MoneyMinor::new(500),
                position(10),
                "SYNTH",
                &orders,
                PriceAtoms::new(100),
                profile(),
            ),
            Err(RiskError::InsufficientMargin)
        );
        assert_eq!(state, before);
    }

    #[test]
    fn working_orders_credit_closing_capacity_only_once() {
        let mut orders = OrderState::new();
        submit_order(&mut orders, "buy-1", Side::Buy, 5, false, -5);
        submit_order(&mut orders, "buy-2", Side::Buy, 5, false, -5);
        let snapshot = margin_snapshot(
            MoneyMinor::new(10_000),
            position(-5),
            "SYNTH",
            &orders,
            PriceAtoms::new(100),
            leverage(2),
            profile(),
        )
        .unwrap();
        assert_eq!(snapshot.position_initial_margin, MoneyMinor::new(250));
        assert_eq!(snapshot.working_order_margin, MoneyMinor::new(250));
    }

    #[test]
    fn reduce_only_orders_reserve_no_expansion_margin() {
        let mut orders = OrderState::new();
        submit_order(&mut orders, "reduce", Side::Sell, 5, true, 5);
        let snapshot = margin_snapshot(
            MoneyMinor::new(1_000),
            position(5),
            "SYNTH",
            &orders,
            PriceAtoms::new(100),
            leverage(2),
            profile(),
        )
        .unwrap();
        assert_eq!(snapshot.working_order_margin, MoneyMinor::new(0));
    }

    #[test]
    fn reversal_always_accepts_close_then_caps_residual_open() {
        let decision = precheck_fill(
            MoneyMinor::new(150),
            position(5),
            Side::Sell,
            QtyAtoms::new(8),
            PriceAtoms::new(100),
            MoneyMinor::new(0),
            leverage(2),
            profile(),
        )
        .unwrap();
        assert_eq!(decision.close_quantity, QtyAtoms::new(5));
        assert_eq!(decision.requested_open_quantity, QtyAtoms::new(3));
        assert_eq!(decision.accepted_open_quantity, QtyAtoms::new(3));
        assert_eq!(decision.accepted_quantity, QtyAtoms::new(8));

        let constrained = precheck_fill(
            MoneyMinor::new(75),
            position(5),
            Side::Sell,
            QtyAtoms::new(8),
            PriceAtoms::new(100),
            MoneyMinor::new(0),
            leverage(2),
            profile(),
        )
        .unwrap();
        assert_eq!(constrained.close_quantity, QtyAtoms::new(5));
        assert_eq!(constrained.accepted_open_quantity, QtyAtoms::new(1));
        assert_eq!(constrained.accepted_quantity, QtyAtoms::new(6));
        assert!(!constrained.fully_accepted());
    }

    #[test]
    fn pure_close_passes_even_when_equity_is_below_current_initial_margin() {
        let decision = precheck_fill(
            MoneyMinor::new(1),
            position(5),
            Side::Sell,
            QtyAtoms::new(2),
            PriceAtoms::new(100),
            MoneyMinor::new(0),
            leverage(2),
            profile(),
        )
        .unwrap();
        assert_eq!(decision.accepted_quantity, QtyAtoms::new(2));
        assert!(decision.fully_accepted());
    }

    #[test]
    fn funding_like_equity_drop_can_trigger_liquidation_deterministically() {
        let mut state = RiskState::new(leverage(2));
        let orders = OrderState::new();
        let position = position(4);
        let before = margin_snapshot(
            MoneyMinor::new(101),
            position,
            "SYNTH",
            &orders,
            PriceAtoms::new(100),
            state.leverage(),
            profile(),
        )
        .unwrap();
        assert_eq!(before.maintenance_margin, MoneyMinor::new(100));
        assert!(!state.evaluate_liquidation(position, before));

        let after_funding = margin_snapshot(
            MoneyMinor::new(99),
            position,
            "SYNTH",
            &orders,
            PriceAtoms::new(100),
            state.leverage(),
            profile(),
        )
        .unwrap();
        assert!(state.evaluate_liquidation(position, after_funding));
        assert_eq!(state.liquidation_state(), LiquidationState::Triggered);
    }

    #[test]
    fn liquidation_completion_requires_flat_position() {
        let mut state = RiskState::new(leverage(2));
        let orders = OrderState::new();
        let position = position(4);
        let snapshot = margin_snapshot(
            MoneyMinor::new(100),
            position,
            "SYNTH",
            &orders,
            PriceAtoms::new(100),
            state.leverage(),
            profile(),
        )
        .unwrap();
        assert!(state.evaluate_liquidation(position, snapshot));
        state.begin_liquidation().unwrap();
        assert_eq!(
            state.complete_liquidation(position),
            Err(RiskError::InvalidLiquidationTransition)
        );
        state.complete_liquidation(Position::flat()).unwrap();
        assert_eq!(state.liquidation_state(), LiquidationState::Liquidated);
    }
}
