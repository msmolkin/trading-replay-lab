//! Deterministic order validation and lifecycle state machine.

use std::collections::BTreeMap;

use crate::numeric::{PriceAtoms, QtyAtoms};

/// Stable simulator-owned order identifier.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct OrderId(u64);

impl OrderId {
    /// Returns the monotonically assigned identifier value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Trading side.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    /// Buy/base-quantity increase.
    Buy,
    /// Sell/base-quantity decrease.
    Sell,
}

/// Time-in-force supported by the canonical v1 order contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeInForce {
    /// Remain working until filled or cancelled.
    Gtc,
    /// Execute available quantity immediately and cancel the remainder.
    Ioc,
    /// Fill the entire remaining quantity immediately or fill nothing.
    Fok,
}

/// Validated order kind. Price requirements are encoded in the type itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderKind {
    /// Market order.
    Market,
    /// Limit order.
    Limit { limit_price: PriceAtoms },
    /// Stop that converts to market after its trigger.
    StopMarket { stop_price: PriceAtoms },
    /// Stop that converts to limit after its trigger.
    StopLimit {
        stop_price: PriceAtoms,
        limit_price: PriceAtoms,
    },
}

impl OrderKind {
    const fn is_stop(self) -> bool {
        matches!(self, Self::StopMarket { .. } | Self::StopLimit { .. })
    }

    const fn limit_price(self) -> Option<PriceAtoms> {
        match self {
            Self::Limit { limit_price } | Self::StopLimit { limit_price, .. } => Some(limit_price),
            Self::Market | Self::StopMarket { .. } => None,
        }
    }
}

/// Best visible prices used only to evaluate order-entry liquidity constraints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TopOfBook {
    /// Highest visible bid.
    pub bid: PriceAtoms,
    /// Lowest visible ask.
    pub ask: PriceAtoms,
}

impl TopOfBook {
    /// Creates a non-crossed positive quote.
    ///
    /// # Errors
    /// Returns [`OrderError::InvalidQuote`] for non-positive or crossed prices.
    pub fn new(bid: PriceAtoms, ask: PriceAtoms) -> Result<Self, OrderError> {
        if bid.get() <= 0 || ask.get() <= 0 || bid >= ask {
            return Err(OrderError::InvalidQuote);
        }
        Ok(Self { bid, ask })
    }
}

/// Order lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderStatus {
    /// Stop order is accepted but has not triggered.
    Dormant,
    /// Active order is eligible for execution.
    Working,
    /// Active order has received at least one partial fill.
    PartiallyFilled,
    /// Entire quantity has filled.
    Filled,
    /// Order was cancelled with quantity remaining.
    Cancelled,
    /// Order was rejected after a state-dependent constraint check.
    Rejected,
}

impl OrderStatus {
    /// Returns whether no further mutation is legal.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Filled | Self::Cancelled | Self::Rejected)
    }
}

/// Canonicalized new-order request accepted by this state machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewOrder {
    /// Stable external idempotency/reference key.
    pub client_order_id: String,
    /// Canonical instrument identifier.
    pub instrument_id: String,
    /// Trading side.
    pub side: Side,
    /// Requested total quantity.
    pub quantity: QtyAtoms,
    /// Market/limit/stop semantics.
    pub kind: OrderKind,
    /// Time-in-force.
    pub time_in_force: TimeInForce,
    /// Prevent position expansion and cap live quantity to reducible exposure.
    pub reduce_only: bool,
    /// Reject if the active order would immediately take displayed liquidity.
    pub post_only: bool,
    /// Reject unless the active order would immediately take displayed liquidity.
    pub marketable_only: bool,
    /// Logical event sequence at submission; never wall-clock time.
    pub submitted_at_event_seq: u64,
}

/// Mutable authoritative order snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Order {
    /// Simulator-owned order id.
    pub id: OrderId,
    /// Stable client reference.
    pub client_order_id: String,
    /// Instrument id.
    pub instrument_id: String,
    /// Side.
    pub side: Side,
    /// Current total order quantity after any replacement/cap.
    pub quantity: QtyAtoms,
    /// Cumulative filled quantity.
    pub filled: QtyAtoms,
    /// Current order kind; stop kinds convert on trigger.
    pub kind: OrderKind,
    /// Time-in-force.
    pub time_in_force: TimeInForce,
    /// Reduce-only flag.
    pub reduce_only: bool,
    /// Post-only flag.
    pub post_only: bool,
    /// Marketable-only flag.
    pub marketable_only: bool,
    /// Submission event sequence.
    pub submitted_at_event_seq: u64,
    /// Replacement revision, starting at zero.
    pub revision: u64,
    /// Lifecycle state.
    pub status: OrderStatus,
}

impl Order {
    /// Returns the unfilled quantity exactly.
    #[must_use]
    pub fn remaining(&self) -> QtyAtoms {
        QtyAtoms::new(self.quantity.get() - self.filled.get())
    }

    /// Returns whether execution may currently consume this order.
    #[must_use]
    pub const fn is_executable(&self) -> bool {
        matches!(
            self.status,
            OrderStatus::Working | OrderStatus::PartiallyFilled
        )
    }
}

/// Replacement fields. Filled quantity and identity are immutable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplaceOrder {
    /// New total quantity, including already-filled quantity.
    pub quantity: QtyAtoms,
    /// New market/limit/stop semantics.
    pub kind: OrderKind,
}

/// Stable fail-closed order errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrderError {
    /// Quantity must be non-zero.
    ZeroQuantity,
    /// Price fields encoded in the order kind are invalid.
    InvalidPrice,
    /// Quote used for entry constraints is invalid.
    InvalidQuote,
    /// Post-only and marketable-only are mutually exclusive.
    ConflictingLiquidityConstraints,
    /// Post-only is not meaningful for a market-style order.
    PostOnlyMarketOrder,
    /// A visible quote was required to validate a liquidity constraint.
    QuoteRequired,
    /// Post-only order would immediately cross the quote.
    PostOnlyWouldTake,
    /// Marketable-only order would not immediately cross the quote.
    MarketableOnlyWouldRest,
    /// Reduce-only order does not reduce the supplied live position.
    ReduceOnlyWrongSide,
    /// No reducible position capacity remains after other live reduce-only orders.
    ReduceOnlyCapacityExhausted,
    /// Order identifier is unknown.
    UnknownOrder,
    /// Requested operation is illegal for the current lifecycle state.
    InvalidState,
    /// Replacement total quantity is below cumulative fills.
    ReplaceBelowFilled,
    /// Fill exceeds remaining quantity.
    Overfill,
    /// Generic fills cannot partially fill FOK orders.
    PartialFokFill,
    /// Immediate execution was requested for a GTC order.
    NotImmediateOrder,
    /// A generic GTC fill was requested for an IOC/FOK order.
    ImmediateOrderRequiresAtomicExecution,
    /// Arithmetic/id/revision counter exhausted.
    CounterOverflow,
}

impl core::fmt::Display for OrderError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let message = match self {
            Self::ZeroQuantity => "order quantity must be positive",
            Self::InvalidPrice => "order price must be positive",
            Self::InvalidQuote => "top-of-book quote is invalid",
            Self::ConflictingLiquidityConstraints => {
                "post-only and marketable-only cannot both be enabled"
            }
            Self::PostOnlyMarketOrder => "post-only is invalid for a market-style order",
            Self::QuoteRequired => "visible quote required for liquidity constraint",
            Self::PostOnlyWouldTake => "post-only order would take displayed liquidity",
            Self::MarketableOnlyWouldRest => "marketable-only order would rest",
            Self::ReduceOnlyWrongSide => "reduce-only order does not reduce live position",
            Self::ReduceOnlyCapacityExhausted => "reduce-only live capacity exhausted",
            Self::UnknownOrder => "unknown order",
            Self::InvalidState => "order operation is invalid in current state",
            Self::ReplaceBelowFilled => "replacement quantity is below cumulative fills",
            Self::Overfill => "fill exceeds remaining order quantity",
            Self::PartialFokFill => "FOK order cannot be partially filled",
            Self::NotImmediateOrder => "order is not IOC or FOK",
            Self::ImmediateOrderRequiresAtomicExecution => {
                "IOC/FOK orders require atomic immediate execution"
            }
            Self::CounterOverflow => "order state counter overflow",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for OrderError {}

/// Outcome of an accepted submission. Reduce-only quantity may be capped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubmitOutcome {
    /// New order id.
    pub order_id: OrderId,
    /// Quantity actually admitted to the live state machine.
    pub accepted_quantity: QtyAtoms,
    /// Original quantity before reduce-only live-cap enforcement.
    pub requested_quantity: QtyAtoms,
}

/// Result of an atomic IOC/FOK execution attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImmediateExecution {
    /// Quantity actually filled.
    pub filled: QtyAtoms,
    /// Final status after the attempt.
    pub status: OrderStatus,
}

/// Result of checking one dormant stop against a visible trigger price.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerOutcome {
    /// Trigger condition was not reached.
    NotTriggered,
    /// Stop converted to an active market/limit order.
    Activated,
    /// Stop triggered but its liquidity constraint rejected the converted order.
    Rejected,
}

/// Deterministic collection and transition engine for all live orders.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrderState {
    next_order_id: u64,
    orders: BTreeMap<OrderId, Order>,
}

impl Default for OrderState {
    fn default() -> Self {
        Self::new()
    }
}

impl OrderState {
    /// Creates empty order state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_order_id: 0,
            orders: BTreeMap::new(),
        }
    }

    /// Returns an order snapshot by stable identifier.
    #[must_use]
    pub fn get(&self, id: OrderId) -> Option<&Order> {
        self.orders.get(&id)
    }

    /// Iterates orders in deterministic id order.
    pub fn iter(&self) -> impl Iterator<Item = &Order> {
        self.orders.values()
    }

    /// Validates and inserts one order.
    ///
    /// `position_atoms` is signed base exposure before this order. Reduce-only requests
    /// are capped against the remaining live reducible quantity after existing live
    /// reduce-only orders on the same instrument/side.
    ///
    /// # Errors
    /// Returns a stable validation error without mutating state.
    pub fn submit(
        &mut self,
        request: NewOrder,
        position_atoms: i64,
        quote: Option<TopOfBook>,
    ) -> Result<SubmitOutcome, OrderError> {
        Self::validate_request(&request)?;
        Self::validate_liquidity_constraint(
            request.side,
            request.kind,
            request.post_only,
            request.marketable_only,
            quote,
        )?;

        let requested_quantity = request.quantity;
        let accepted_quantity = if request.reduce_only {
            self.reduce_only_quantity(
                &request.instrument_id,
                request.side,
                requested_quantity,
                position_atoms,
                None,
            )?
        } else {
            requested_quantity
        };

        let next = self
            .next_order_id
            .checked_add(1)
            .ok_or(OrderError::CounterOverflow)?;
        let id = OrderId(self.next_order_id);
        let status = if request.kind.is_stop() {
            OrderStatus::Dormant
        } else {
            OrderStatus::Working
        };
        let order = Order {
            id,
            client_order_id: request.client_order_id,
            instrument_id: request.instrument_id,
            side: request.side,
            quantity: accepted_quantity,
            filled: QtyAtoms::new(0),
            kind: request.kind,
            time_in_force: request.time_in_force,
            reduce_only: request.reduce_only,
            post_only: request.post_only,
            marketable_only: request.marketable_only,
            submitted_at_event_seq: request.submitted_at_event_seq,
            revision: 0,
            status,
        };
        self.orders.insert(id, order);
        self.next_order_id = next;
        Ok(SubmitOutcome {
            order_id: id,
            accepted_quantity,
            requested_quantity,
        })
    }

    /// Cancels a non-terminal order. Dormant stops may be cancelled before triggering.
    ///
    /// # Errors
    /// Returns [`OrderError::UnknownOrder`] or [`OrderError::InvalidState`] without mutation.
    pub fn cancel(&mut self, id: OrderId) -> Result<(), OrderError> {
        let order = self.orders.get_mut(&id).ok_or(OrderError::UnknownOrder)?;
        if order.status.is_terminal() {
            return Err(OrderError::InvalidState);
        }
        order.status = OrderStatus::Cancelled;
        Ok(())
    }

    /// Replaces quantity/kind atomically while preserving fills and order identity.
    ///
    /// # Errors
    /// Returns a validation/capacity/state error without mutating the existing order.
    pub fn replace(
        &mut self,
        id: OrderId,
        replacement: ReplaceOrder,
        position_atoms: i64,
        quote: Option<TopOfBook>,
    ) -> Result<QtyAtoms, OrderError> {
        let existing = self
            .orders
            .get(&id)
            .ok_or(OrderError::UnknownOrder)?
            .clone();
        if existing.status.is_terminal() {
            return Err(OrderError::InvalidState);
        }
        if replacement.quantity.get() == 0 {
            return Err(OrderError::ZeroQuantity);
        }
        Self::validate_kind(replacement.kind)?;
        if replacement.quantity < existing.filled {
            return Err(OrderError::ReplaceBelowFilled);
        }
        Self::validate_liquidity_constraint(
            existing.side,
            replacement.kind,
            existing.post_only,
            existing.marketable_only,
            quote,
        )?;

        let desired_remaining = QtyAtoms::new(replacement.quantity.get() - existing.filled.get());
        if desired_remaining.get() == 0 {
            return Err(OrderError::ReplaceBelowFilled);
        }
        let accepted_remaining = if existing.reduce_only {
            self.reduce_only_quantity(
                &existing.instrument_id,
                existing.side,
                desired_remaining,
                position_atoms,
                Some(id),
            )?
        } else {
            desired_remaining
        };
        let accepted_total = existing
            .filled
            .get()
            .checked_add(accepted_remaining.get())
            .map(QtyAtoms::new)
            .ok_or(OrderError::CounterOverflow)?;
        let next_revision = existing
            .revision
            .checked_add(1)
            .ok_or(OrderError::CounterOverflow)?;

        let order = self.orders.get_mut(&id).ok_or(OrderError::UnknownOrder)?;
        order.quantity = accepted_total;
        order.kind = replacement.kind;
        order.revision = next_revision;
        order.status = if replacement.kind.is_stop() {
            OrderStatus::Dormant
        } else if order.filled.get() == 0 {
            OrderStatus::Working
        } else {
            OrderStatus::PartiallyFilled
        };
        Ok(accepted_total)
    }

    /// Records a normal execution fill for a working GTC order.
    ///
    /// IOC/FOK orders use [`Self::execute_immediate`] so their terminal semantics are atomic.
    ///
    /// # Errors
    /// Returns a state/quantity error without mutation.
    pub fn record_fill(
        &mut self,
        id: OrderId,
        quantity: QtyAtoms,
    ) -> Result<OrderStatus, OrderError> {
        let order = self.orders.get_mut(&id).ok_or(OrderError::UnknownOrder)?;
        if !order.is_executable() {
            return Err(OrderError::InvalidState);
        }
        if order.time_in_force != TimeInForce::Gtc {
            return Err(OrderError::ImmediateOrderRequiresAtomicExecution);
        }
        Self::apply_fill(order, quantity, false)?;
        Ok(order.status)
    }

    /// Applies one complete liquidity attempt atomically to an IOC or FOK order.
    ///
    /// FOK fills all remaining quantity if enough liquidity is available or fills nothing.
    /// IOC consumes up to the remaining quantity and cancels any unfilled remainder.
    ///
    /// # Errors
    /// Returns a state error without mutation.
    pub fn execute_immediate(
        &mut self,
        id: OrderId,
        available_fill: QtyAtoms,
    ) -> Result<ImmediateExecution, OrderError> {
        let order = self.orders.get_mut(&id).ok_or(OrderError::UnknownOrder)?;
        if !order.is_executable() {
            return Err(OrderError::InvalidState);
        }
        if order.time_in_force == TimeInForce::Gtc {
            return Err(OrderError::NotImmediateOrder);
        }
        let remaining = order.remaining();
        let fillable = QtyAtoms::new(available_fill.get().min(remaining.get()));

        match order.time_in_force {
            TimeInForce::Fok if available_fill < remaining => {
                order.status = OrderStatus::Cancelled;
                Ok(ImmediateExecution {
                    filled: QtyAtoms::new(0),
                    status: order.status,
                })
            }
            TimeInForce::Fok => {
                Self::apply_fill(order, remaining, true)?;
                Ok(ImmediateExecution {
                    filled: remaining,
                    status: order.status,
                })
            }
            TimeInForce::Ioc => {
                if fillable.get() > 0 {
                    Self::apply_fill(order, fillable, true)?;
                }
                if !order.status.is_terminal() {
                    order.status = OrderStatus::Cancelled;
                }
                Ok(ImmediateExecution {
                    filled: fillable,
                    status: order.status,
                })
            }
            TimeInForce::Gtc => unreachable!("GTC rejected above"),
        }
    }

    /// Checks and converts a dormant stop using only the supplied visible trigger price.
    ///
    /// Buy stops trigger at or above the stop; sell stops trigger at or below it. A
    /// triggered stop-market becomes market; a stop-limit becomes limit. Liquidity
    /// constraints are re-evaluated at conversion and can deterministically reject it.
    ///
    /// # Errors
    /// Returns a stable state/quote error without partial conversion.
    pub fn trigger_stop(
        &mut self,
        id: OrderId,
        trigger_price: PriceAtoms,
        quote: Option<TopOfBook>,
    ) -> Result<TriggerOutcome, OrderError> {
        if trigger_price.get() <= 0 {
            return Err(OrderError::InvalidPrice);
        }
        let existing = self
            .orders
            .get(&id)
            .ok_or(OrderError::UnknownOrder)?
            .clone();
        if existing.status != OrderStatus::Dormant {
            return Err(OrderError::InvalidState);
        }
        let (stop_price, converted) = match existing.kind {
            OrderKind::StopMarket { stop_price } => (stop_price, OrderKind::Market),
            OrderKind::StopLimit {
                stop_price,
                limit_price,
            } => (stop_price, OrderKind::Limit { limit_price }),
            OrderKind::Market | OrderKind::Limit { .. } => return Err(OrderError::InvalidState),
        };
        let triggered = match existing.side {
            Side::Buy => trigger_price >= stop_price,
            Side::Sell => trigger_price <= stop_price,
        };
        if !triggered {
            return Ok(TriggerOutcome::NotTriggered);
        }

        if Self::validate_liquidity_constraint(
            existing.side,
            converted,
            existing.post_only,
            existing.marketable_only,
            quote,
        )
        .is_err()
        {
            let order = self.orders.get_mut(&id).ok_or(OrderError::UnknownOrder)?;
            order.status = OrderStatus::Rejected;
            return Ok(TriggerOutcome::Rejected);
        }

        let order = self.orders.get_mut(&id).ok_or(OrderError::UnknownOrder)?;
        order.kind = converted;
        order.status = if order.filled.get() == 0 {
            OrderStatus::Working
        } else {
            OrderStatus::PartiallyFilled
        };
        Ok(TriggerOutcome::Activated)
    }

    fn validate_request(request: &NewOrder) -> Result<(), OrderError> {
        if request.quantity.get() == 0 {
            return Err(OrderError::ZeroQuantity);
        }
        if request.client_order_id.is_empty() || request.instrument_id.is_empty() {
            return Err(OrderError::InvalidState);
        }
        if request.post_only && request.marketable_only {
            return Err(OrderError::ConflictingLiquidityConstraints);
        }
        Self::validate_kind(request.kind)?;
        if request.post_only
            && matches!(
                request.kind,
                OrderKind::Market | OrderKind::StopMarket { .. }
            )
        {
            return Err(OrderError::PostOnlyMarketOrder);
        }
        Ok(())
    }

    fn validate_kind(kind: OrderKind) -> Result<(), OrderError> {
        let prices_valid = match kind {
            OrderKind::Market => true,
            OrderKind::Limit { limit_price } => limit_price.get() > 0,
            OrderKind::StopMarket { stop_price } => stop_price.get() > 0,
            OrderKind::StopLimit {
                stop_price,
                limit_price,
            } => stop_price.get() > 0 && limit_price.get() > 0,
        };
        if prices_valid {
            Ok(())
        } else {
            Err(OrderError::InvalidPrice)
        }
    }

    fn validate_liquidity_constraint(
        side: Side,
        kind: OrderKind,
        post_only: bool,
        marketable_only: bool,
        quote: Option<TopOfBook>,
    ) -> Result<(), OrderError> {
        if kind.is_stop() {
            return Ok(());
        }
        if !post_only && !marketable_only {
            return Ok(());
        }
        let quote = quote.ok_or(OrderError::QuoteRequired)?;
        let marketable = Self::is_marketable(side, kind, quote);
        if post_only && marketable {
            return Err(OrderError::PostOnlyWouldTake);
        }
        if marketable_only && !marketable {
            return Err(OrderError::MarketableOnlyWouldRest);
        }
        Ok(())
    }

    fn is_marketable(side: Side, kind: OrderKind, quote: TopOfBook) -> bool {
        match kind.limit_price() {
            None => matches!(kind, OrderKind::Market),
            Some(limit) => match side {
                Side::Buy => limit >= quote.ask,
                Side::Sell => limit <= quote.bid,
            },
        }
    }

    fn reduce_only_quantity(
        &self,
        instrument_id: &str,
        side: Side,
        requested: QtyAtoms,
        position_atoms: i64,
        excluding: Option<OrderId>,
    ) -> Result<QtyAtoms, OrderError> {
        let reducible = match (position_atoms.cmp(&0), side) {
            (core::cmp::Ordering::Greater, Side::Sell) | (core::cmp::Ordering::Less, Side::Buy) => {
                position_atoms.unsigned_abs()
            }
            _ => return Err(OrderError::ReduceOnlyWrongSide),
        };
        let reserved = self
            .orders
            .values()
            .filter(|order| {
                Some(order.id) != excluding
                    && order.instrument_id == instrument_id
                    && order.side == side
                    && order.reduce_only
                    && !order.status.is_terminal()
            })
            .try_fold(0_u64, |total, order| {
                total
                    .checked_add(order.remaining().get())
                    .ok_or(OrderError::CounterOverflow)
            })?;
        let available = reducible.saturating_sub(reserved);
        if available == 0 {
            return Err(OrderError::ReduceOnlyCapacityExhausted);
        }
        Ok(QtyAtoms::new(requested.get().min(available)))
    }

    fn apply_fill(
        order: &mut Order,
        quantity: QtyAtoms,
        allow_fok: bool,
    ) -> Result<(), OrderError> {
        if quantity.get() == 0 {
            return Ok(());
        }
        let remaining = order.remaining();
        if quantity > remaining {
            return Err(OrderError::Overfill);
        }
        if order.time_in_force == TimeInForce::Fok && quantity != remaining && !allow_fok {
            return Err(OrderError::PartialFokFill);
        }
        order.filled = QtyAtoms::new(
            order
                .filled
                .get()
                .checked_add(quantity.get())
                .ok_or(OrderError::CounterOverflow)?,
        );
        order.status = if order.filled == order.quantity {
            OrderStatus::Filled
        } else {
            OrderStatus::PartiallyFilled
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(side: Side, price: i64, tif: TimeInForce) -> NewOrder {
        NewOrder {
            client_order_id: "client-1".into(),
            instrument_id: "SYNTH".into(),
            side,
            quantity: QtyAtoms::new(10),
            kind: OrderKind::Limit {
                limit_price: PriceAtoms::new(price),
            },
            time_in_force: tif,
            reduce_only: false,
            post_only: false,
            marketable_only: false,
            submitted_at_event_seq: 7,
        }
    }

    fn quote() -> TopOfBook {
        TopOfBook::new(PriceAtoms::new(99), PriceAtoms::new(101)).unwrap()
    }

    #[test]
    fn lifecycle_and_quantity_conservation() {
        let mut state = OrderState::new();
        let id = state
            .submit(limit(Side::Buy, 100, TimeInForce::Gtc), 0, None)
            .unwrap()
            .order_id;
        assert_eq!(state.get(id).unwrap().status, OrderStatus::Working);
        assert_eq!(
            state.record_fill(id, QtyAtoms::new(4)).unwrap(),
            OrderStatus::PartiallyFilled
        );
        let order = state.get(id).unwrap();
        assert_eq!(
            order.filled.get() + order.remaining().get(),
            order.quantity.get()
        );
        assert_eq!(
            state.record_fill(id, QtyAtoms::new(6)).unwrap(),
            OrderStatus::Filled
        );
        assert_eq!(state.cancel(id), Err(OrderError::InvalidState));
    }

    #[test]
    fn replace_preserves_fills_and_identity() {
        let mut state = OrderState::new();
        let id = state
            .submit(limit(Side::Sell, 105, TimeInForce::Gtc), 0, None)
            .unwrap()
            .order_id;
        state.record_fill(id, QtyAtoms::new(3)).unwrap();
        let total = state
            .replace(
                id,
                ReplaceOrder {
                    quantity: QtyAtoms::new(8),
                    kind: OrderKind::Limit {
                        limit_price: PriceAtoms::new(104),
                    },
                },
                0,
                None,
            )
            .unwrap();
        let kind = state.get(id).unwrap().kind;
        let order = state.get(id).unwrap();
        assert_eq!(total.get(), 8);
        assert_eq!(order.filled.get(), 3);
        assert_eq!(order.remaining().get(), 5);
        assert_eq!(order.revision, 1);
        assert_eq!(
            state.replace(
                id,
                ReplaceOrder {
                    quantity: QtyAtoms::new(2),
                    kind,
                },
                0,
                None,
            ),
            Err(OrderError::ReplaceBelowFilled)
        );
    }

    #[test]
    fn reduce_only_caps_across_live_orders() {
        let mut state = OrderState::new();
        let mut request = limit(Side::Sell, 105, TimeInForce::Gtc);
        request.reduce_only = true;
        request.quantity = QtyAtoms::new(7);
        let first = state.submit(request.clone(), 10, None).unwrap();
        assert_eq!(first.accepted_quantity.get(), 7);
        request.client_order_id = "client-2".into();
        request.quantity = QtyAtoms::new(7);
        let second = state.submit(request.clone(), 10, None).unwrap();
        assert_eq!(second.accepted_quantity.get(), 3);
        request.client_order_id = "client-3".into();
        assert_eq!(
            state.submit(request, 10, None),
            Err(OrderError::ReduceOnlyCapacityExhausted)
        );
    }

    #[test]
    fn reduce_only_wrong_side_fails_without_insertion() {
        let mut state = OrderState::new();
        let mut request = limit(Side::Buy, 100, TimeInForce::Gtc);
        request.reduce_only = true;
        assert_eq!(
            state.submit(request, 5, None),
            Err(OrderError::ReduceOnlyWrongSide)
        );
        assert_eq!(state.iter().count(), 0);
    }

    #[test]
    fn post_only_and_marketable_only_use_only_visible_quote() {
        let mut state = OrderState::new();
        let mut post = limit(Side::Buy, 101, TimeInForce::Gtc);
        post.post_only = true;
        assert_eq!(
            state.submit(post.clone(), 0, Some(quote())),
            Err(OrderError::PostOnlyWouldTake)
        );
        post.kind = OrderKind::Limit {
            limit_price: PriceAtoms::new(100),
        };
        assert!(state.submit(post, 0, Some(quote())).is_ok());

        let mut marketable = limit(Side::Sell, 100, TimeInForce::Gtc);
        marketable.marketable_only = true;
        assert_eq!(
            state.submit(marketable.clone(), 0, Some(quote())),
            Err(OrderError::MarketableOnlyWouldRest)
        );
        marketable.kind = OrderKind::Limit {
            limit_price: PriceAtoms::new(99),
        };
        assert!(state.submit(marketable, 0, Some(quote())).is_ok());
    }

    #[test]
    fn ioc_and_fok_are_atomic() {
        let mut state = OrderState::new();
        let ioc = state
            .submit(limit(Side::Buy, 101, TimeInForce::Ioc), 0, None)
            .unwrap()
            .order_id;
        let ioc_result = state.execute_immediate(ioc, QtyAtoms::new(4)).unwrap();
        assert_eq!(ioc_result.filled.get(), 4);
        assert_eq!(ioc_result.status, OrderStatus::Cancelled);
        assert_eq!(state.get(ioc).unwrap().filled.get(), 4);

        let ioc_excess = state
            .submit(limit(Side::Buy, 101, TimeInForce::Ioc), 0, None)
            .unwrap()
            .order_id;
        let ioc_excess_result = state
            .execute_immediate(ioc_excess, QtyAtoms::new(40))
            .unwrap();
        assert_eq!(ioc_excess_result.filled.get(), 10);
        assert_eq!(ioc_excess_result.status, OrderStatus::Filled);

        let fok = state
            .submit(limit(Side::Buy, 101, TimeInForce::Fok), 0, None)
            .unwrap()
            .order_id;
        let failed = state.execute_immediate(fok, QtyAtoms::new(9)).unwrap();
        assert_eq!(failed.filled.get(), 0);
        assert_eq!(failed.status, OrderStatus::Cancelled);
        assert_eq!(state.get(fok).unwrap().filled.get(), 0);

        let full = state
            .submit(limit(Side::Buy, 101, TimeInForce::Fok), 0, None)
            .unwrap()
            .order_id;
        let filled = state.execute_immediate(full, QtyAtoms::new(11)).unwrap();
        assert_eq!(filled.filled.get(), 10);
        assert_eq!(filled.status, OrderStatus::Filled);
    }

    #[test]
    fn immediate_orders_cannot_use_generic_fill_path() {
        let mut state = OrderState::new();
        let id = state
            .submit(limit(Side::Buy, 101, TimeInForce::Ioc), 0, None)
            .unwrap()
            .order_id;
        assert_eq!(
            state.record_fill(id, QtyAtoms::new(1)),
            Err(OrderError::ImmediateOrderRequiresAtomicExecution)
        );
        assert_eq!(state.get(id).unwrap().filled.get(), 0);
    }

    #[test]
    fn stop_trigger_conversion_is_directional_and_constraint_checked() {
        let mut state = OrderState::new();
        let mut stop = limit(Side::Buy, 100, TimeInForce::Gtc);
        stop.kind = OrderKind::StopLimit {
            stop_price: PriceAtoms::new(105),
            limit_price: PriceAtoms::new(100),
        };
        stop.post_only = true;
        let id = state.submit(stop, 0, None).unwrap().order_id;
        assert_eq!(
            state
                .trigger_stop(id, PriceAtoms::new(104), Some(quote()))
                .unwrap(),
            TriggerOutcome::NotTriggered
        );
        assert_eq!(
            state
                .trigger_stop(id, PriceAtoms::new(105), Some(quote()))
                .unwrap(),
            TriggerOutcome::Activated
        );
        let order = state.get(id).unwrap();
        assert_eq!(
            order.kind,
            OrderKind::Limit {
                limit_price: PriceAtoms::new(100)
            }
        );
        assert_eq!(order.status, OrderStatus::Working);
    }

    #[test]
    fn triggered_stop_can_reject_post_only_cross() {
        let mut state = OrderState::new();
        let request = NewOrder {
            client_order_id: "stop".into(),
            instrument_id: "SYNTH".into(),
            side: Side::Buy,
            quantity: QtyAtoms::new(1),
            kind: OrderKind::StopLimit {
                stop_price: PriceAtoms::new(101),
                limit_price: PriceAtoms::new(101),
            },
            time_in_force: TimeInForce::Gtc,
            reduce_only: false,
            post_only: true,
            marketable_only: false,
            submitted_at_event_seq: 1,
        };
        let id = state.submit(request, 0, None).unwrap().order_id;
        assert_eq!(
            state
                .trigger_stop(id, PriceAtoms::new(101), Some(quote()))
                .unwrap(),
            TriggerOutcome::Rejected
        );
        assert_eq!(state.get(id).unwrap().status, OrderStatus::Rejected);
    }

    #[test]
    fn invalid_entry_is_fail_closed() {
        let mut state = OrderState::new();
        let mut request = limit(Side::Buy, 100, TimeInForce::Gtc);
        request.quantity = QtyAtoms::new(0);
        assert_eq!(
            state.submit(request, 0, None),
            Err(OrderError::ZeroQuantity)
        );
        assert_eq!(state.iter().count(), 0);
    }
}
