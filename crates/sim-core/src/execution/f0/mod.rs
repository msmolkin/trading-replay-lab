//! F0 deterministic execution using completed OHLCV bars only.

use crate::hash::sha256;
use crate::numeric::{PriceAtoms, QtyAtoms};
use crate::orders::{
    OrderError, OrderId, OrderKind, OrderState, OrderStatus, Side, TimeInForce, TriggerOutcome,
};

/// One completed canonical bar supplied to F0.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Bar {
    /// Logical market-event sequence. Orders submitted at this sequence are not eligible.
    pub event_seq: u64,
    /// Opening price.
    pub open: PriceAtoms,
    /// Highest price observed in the bar.
    pub high: PriceAtoms,
    /// Lowest price observed in the bar.
    pub low: PriceAtoms,
    /// Closing price.
    pub close: PriceAtoms,
    /// Observed base quantity; informational in F0 v1.
    pub base_volume: QtyAtoms,
}

impl Bar {
    /// Validates positive coherent OHLC values.
    ///
    /// # Errors
    /// Returns [`F0Error::InvalidBar`] if the OHLC envelope is impossible.
    pub fn validate(self) -> Result<(), F0Error> {
        let open = self.open.get();
        let high = self.high.get();
        let low = self.low.get();
        let close = self.close.get();
        if open <= 0
            || high <= 0
            || low <= 0
            || close <= 0
            || low > open
            || low > close
            || high < open
            || high < close
            || low > high
        {
            return Err(F0Error::InvalidBar);
        }
        Ok(())
    }
}

/// Policy for OHLC bars where event ordering inside the interval is unknowable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntrabarPolicy {
    /// Choose the player-adverse result when both outcomes are consistent with the bar.
    Pessimistic,
    /// Choose the player-favorable result when both outcomes are consistent with the bar.
    Optimistic,
    /// Choose deterministically from a committed seed and event/order identity.
    Seeded { seed: u64 },
}

/// Explicit uncertainty attached to a bar-derived decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UncertaintyFlag {
    /// OHLC values prove both relevant levels were reached but not which came first.
    IntrabarAmbiguous,
}

/// Binary resolution for an otherwise unknowable intrabar ordering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmbiguousChoice {
    /// Select the outcome adverse to the player.
    Adverse,
    /// Select the outcome favorable to the player.
    Favorable,
}

/// F0 configuration committed as part of simulator rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct F0Config {
    /// Ambiguous intrabar ordering policy.
    pub intrabar_policy: IntrabarPolicy,
    /// Fixed adverse slippage in price atoms for market-style fills.
    pub market_slippage_atoms: u64,
}

impl Default for F0Config {
    fn default() -> Self {
        Self {
            intrabar_policy: IntrabarPolicy::Pessimistic,
            market_slippage_atoms: 0,
        }
    }
}

/// Deterministic result of offering one order one completed bar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct F0Outcome {
    /// Order evaluated.
    pub order_id: OrderId,
    /// Quantity filled on this bar.
    pub filled: QtyAtoms,
    /// Execution price when a fill occurred.
    pub fill_price: Option<PriceAtoms>,
    /// Whether a dormant stop activated on this bar.
    pub triggered: bool,
    /// Final authoritative order status after processing.
    pub status: OrderStatus,
    /// Explicit model uncertainty; never inferred by callers.
    pub uncertainty: Vec<UncertaintyFlag>,
}

/// Stable F0 failures. All failures occur before any fill mutation unless the underlying
/// order state machine itself rejects a requested atomic transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum F0Error {
    /// Bar OHLC values are impossible or non-positive.
    InvalidBar,
    /// Checked slippage arithmetic overflowed or crossed zero.
    PriceArithmetic,
    /// Order-state transition failed.
    Order(OrderError),
}

impl core::fmt::Display for F0Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidBar => formatter.write_str("invalid F0 OHLC bar"),
            Self::PriceArithmetic => formatter.write_str("F0 price arithmetic failed"),
            Self::Order(error) => write!(formatter, "F0 order transition failed: {error}"),
        }
    }
}

impl std::error::Error for F0Error {}

impl From<OrderError> for F0Error {
    fn from(value: OrderError) -> Self {
        Self::Order(value)
    }
}

/// Applies one completed F0 bar to one order.
///
/// Market orders fill at the first eligible bar open. Limits can fill only if their level
/// lies inside the bar envelope. Stops cannot trigger on a bar whose sequence is not later
/// than submission. Stop-market fills use the opening price when the bar gaps through the
/// stop and otherwise the stop level, with configured adverse slippage. Stop-limit orders
/// that both trigger and reach their limit within one bar are explicitly ambiguous when the
/// relative ordering cannot be inferred; policy selects whether that same-bar limit fill is
/// allowed while the converted order remains live if pessimistic policy declines it.
///
/// # Errors
/// Returns [`F0Error`] without applying a fill for invalid bars/arithmetic/order transitions.
pub fn execute_bar(
    orders: &mut OrderState,
    order_id: OrderId,
    bar: Bar,
    config: F0Config,
) -> Result<F0Outcome, F0Error> {
    bar.validate()?;
    let before = orders
        .get(order_id)
        .ok_or(OrderError::UnknownOrder)?
        .clone();
    if before.status.is_terminal() || bar.event_seq <= before.submitted_at_event_seq {
        return Ok(no_fill(&before, false, Vec::new()));
    }

    let original_kind = before.kind;
    let original_stop = stop_price(original_kind);
    let stop_will_trigger = before.status == OrderStatus::Dormant
        && original_stop.is_some_and(|stop| stop_touched(before.side, stop, bar));
    let precomputed_stop_market_price = if stop_will_trigger
        && matches!(original_kind, OrderKind::StopMarket { .. })
    {
        let base = stop_market_base_price(before.side, original_stop, bar);
        Some(adverse_slippage(
            base,
            before.side,
            config.market_slippage_atoms,
        )?)
    } else {
        None
    };

    let mut triggered = false;
    let mut uncertainty = Vec::new();
    if before.status == OrderStatus::Dormant {
        let Some(stop) = original_stop else {
            return Err(OrderError::InvalidState.into());
        };
        if stop_will_trigger {
            let trigger = orders.trigger_stop(order_id, stop, None)?;
            match trigger {
                TriggerOutcome::Activated => triggered = true,
                TriggerOutcome::Rejected => {
                    let order = orders.get(order_id).ok_or(OrderError::UnknownOrder)?;
                    return Ok(no_fill(order, true, uncertainty));
                }
                TriggerOutcome::NotTriggered => unreachable!("bar touch and stop trigger agree"),
            }
        } else {
            return Ok(no_fill(&before, false, uncertainty));
        }
    }

    let active = orders
        .get(order_id)
        .ok_or(OrderError::UnknownOrder)?
        .clone();
    if !active.is_executable() {
        return Ok(no_fill(&active, triggered, uncertainty));
    }

    let (should_fill, raw_price) = match active.kind {
        OrderKind::Market => {
            let fill_price = if triggered {
                precomputed_stop_market_price
                    .expect("triggered market is converted from precomputed stop-market")
            } else {
                adverse_slippage(bar.open, active.side, config.market_slippage_atoms)?
            };
            (true, fill_price)
        }
        OrderKind::Limit { limit_price } => {
            if !limit_touched(active.side, limit_price, bar) {
                (false, limit_price)
            } else if triggered && matches!(original_kind, OrderKind::StopLimit { .. }) {
                if stop_limit_same_bar_is_ambiguous(before.side, original_kind, bar) {
                    uncertainty.push(UncertaintyFlag::IntrabarAmbiguous);
                    let choice =
                        resolve_ambiguous(config.intrabar_policy, bar.event_seq, order_id.get());
                    (
                        choice == AmbiguousChoice::Favorable,
                        limit_fill_price(active.side, limit_price, bar.open),
                    )
                } else {
                    (true, limit_fill_price(active.side, limit_price, bar.open))
                }
            } else {
                (true, limit_fill_price(active.side, limit_price, bar.open))
            }
        }
        OrderKind::StopMarket { .. } | OrderKind::StopLimit { .. } => {
            return Err(OrderError::InvalidState.into());
        }
    };

    if !should_fill {
        let order = orders.get(order_id).ok_or(OrderError::UnknownOrder)?;
        return Ok(no_fill(order, triggered, uncertainty));
    }

    let remaining = active.remaining();
    let filled = match active.time_in_force {
        TimeInForce::Gtc => {
            orders.record_fill(order_id, remaining)?;
            remaining
        }
        TimeInForce::Ioc | TimeInForce::Fok => {
            orders.execute_immediate(order_id, remaining)?.filled
        }
    };
    let status = orders.get(order_id).ok_or(OrderError::UnknownOrder)?.status;
    Ok(F0Outcome {
        order_id,
        filled,
        fill_price: (filled.get() > 0).then_some(raw_price),
        triggered,
        status,
        uncertainty,
    })
}

/// Resolves an intrinsically ambiguous binary intrabar outcome deterministically.
///
/// The seeded form hashes a domain tag plus seed/event/order identifiers and therefore
/// does not depend on process RNG, iteration ordering, or platform endianness.
#[must_use]
pub fn resolve_ambiguous(policy: IntrabarPolicy, event_seq: u64, identity: u64) -> AmbiguousChoice {
    match policy {
        IntrabarPolicy::Pessimistic => AmbiguousChoice::Adverse,
        IntrabarPolicy::Optimistic => AmbiguousChoice::Favorable,
        IntrabarPolicy::Seeded { seed } => {
            let mut bytes = Vec::with_capacity(32);
            bytes.extend_from_slice(b"trl:f0:intrabar:v1");
            bytes.extend_from_slice(&seed.to_be_bytes());
            bytes.extend_from_slice(&event_seq.to_be_bytes());
            bytes.extend_from_slice(&identity.to_be_bytes());
            if sha256(&bytes)[0] & 1 == 0 {
                AmbiguousChoice::Adverse
            } else {
                AmbiguousChoice::Favorable
            }
        }
    }
}

fn no_fill(
    order: &crate::orders::Order,
    triggered: bool,
    uncertainty: Vec<UncertaintyFlag>,
) -> F0Outcome {
    F0Outcome {
        order_id: order.id,
        filled: QtyAtoms::new(0),
        fill_price: None,
        triggered,
        status: order.status,
        uncertainty,
    }
}

const fn stop_price(kind: OrderKind) -> Option<PriceAtoms> {
    match kind {
        OrderKind::StopMarket { stop_price } | OrderKind::StopLimit { stop_price, .. } => {
            Some(stop_price)
        }
        OrderKind::Market | OrderKind::Limit { .. } => None,
    }
}

fn stop_touched(side: Side, stop: PriceAtoms, bar: Bar) -> bool {
    match side {
        Side::Buy => bar.high >= stop,
        Side::Sell => bar.low <= stop,
    }
}

fn limit_touched(side: Side, limit: PriceAtoms, bar: Bar) -> bool {
    match side {
        Side::Buy => bar.low <= limit,
        Side::Sell => bar.high >= limit,
    }
}

fn limit_fill_price(side: Side, limit: PriceAtoms, open: PriceAtoms) -> PriceAtoms {
    match side {
        Side::Buy => PriceAtoms::new(open.get().min(limit.get())),
        Side::Sell => PriceAtoms::new(open.get().max(limit.get())),
    }
}

fn stop_market_base_price(side: Side, stop: Option<PriceAtoms>, bar: Bar) -> PriceAtoms {
    let stop = stop.expect("triggered stop-market retains original stop");
    match side {
        Side::Buy if bar.open >= stop => bar.open,
        Side::Sell if bar.open <= stop => bar.open,
        Side::Buy | Side::Sell => stop,
    }
}

fn adverse_slippage(price: PriceAtoms, side: Side, atoms: u64) -> Result<PriceAtoms, F0Error> {
    let atoms = i64::try_from(atoms).map_err(|_| F0Error::PriceArithmetic)?;
    let value = match side {
        Side::Buy => price.get().checked_add(atoms),
        Side::Sell => price.get().checked_sub(atoms),
    }
    .ok_or(F0Error::PriceArithmetic)?;
    if value <= 0 {
        return Err(F0Error::PriceArithmetic);
    }
    Ok(PriceAtoms::new(value))
}

fn stop_limit_same_bar_is_ambiguous(side: Side, original: OrderKind, bar: Bar) -> bool {
    let OrderKind::StopLimit {
        stop_price,
        limit_price,
    } = original
    else {
        return false;
    };
    let gap_triggered = match side {
        Side::Buy => bar.open >= stop_price,
        Side::Sell => bar.open <= stop_price,
    };
    if gap_triggered {
        return false;
    }
    match side {
        Side::Buy => limit_price < stop_price && bar.low <= limit_price && bar.high >= stop_price,
        Side::Sell => limit_price > stop_price && bar.high >= limit_price && bar.low <= stop_price,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::{NewOrder, TimeInForce};

    fn bar(event_seq: u64, open: i64, high: i64, low: i64, close: i64) -> Bar {
        Bar {
            event_seq,
            open: PriceAtoms::new(open),
            high: PriceAtoms::new(high),
            low: PriceAtoms::new(low),
            close: PriceAtoms::new(close),
            base_volume: QtyAtoms::new(1_000),
        }
    }

    fn submit(
        orders: &mut OrderState,
        side: Side,
        kind: OrderKind,
        submitted_at_event_seq: u64,
    ) -> OrderId {
        orders
            .submit(
                NewOrder {
                    client_order_id: "f0-order".into(),
                    instrument_id: "SYNTH".into(),
                    side,
                    quantity: QtyAtoms::new(10),
                    kind,
                    time_in_force: TimeInForce::Gtc,
                    reduce_only: false,
                    post_only: false,
                    marketable_only: false,
                    submitted_at_event_seq,
                },
                0,
                None,
            )
            .unwrap()
            .order_id
    }

    #[test]
    fn market_never_fills_submission_bar_and_then_uses_next_open() {
        let mut orders = OrderState::new();
        let id = submit(&mut orders, Side::Buy, OrderKind::Market, 7);
        let prior = execute_bar(
            &mut orders,
            id,
            bar(7, 100, 110, 90, 105),
            F0Config::default(),
        )
        .unwrap();
        assert_eq!(prior.filled, QtyAtoms::new(0));
        let next = execute_bar(
            &mut orders,
            id,
            bar(8, 101, 111, 99, 108),
            F0Config {
                market_slippage_atoms: 2,
                ..F0Config::default()
            },
        )
        .unwrap();
        assert_eq!(next.filled, QtyAtoms::new(10));
        assert_eq!(next.fill_price, Some(PriceAtoms::new(103)));
        assert_eq!(next.status, OrderStatus::Filled);
    }

    #[test]
    fn limit_requires_bar_reach_and_preserves_better_gap_open() {
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            Side::Buy,
            OrderKind::Limit {
                limit_price: PriceAtoms::new(100),
            },
            1,
        );
        assert_eq!(
            execute_bar(
                &mut orders,
                id,
                bar(2, 105, 110, 101, 104),
                F0Config::default(),
            )
            .unwrap()
            .filled,
            QtyAtoms::new(0)
        );
        let fill = execute_bar(
            &mut orders,
            id,
            bar(3, 98, 105, 95, 100),
            F0Config::default(),
        )
        .unwrap();
        assert_eq!(fill.fill_price, Some(PriceAtoms::new(98)));
    }

    #[test]
    fn stop_market_gap_uses_gap_open_plus_adverse_slippage() {
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            Side::Buy,
            OrderKind::StopMarket {
                stop_price: PriceAtoms::new(105),
            },
            1,
        );
        let fill = execute_bar(
            &mut orders,
            id,
            bar(2, 110, 115, 108, 112),
            F0Config {
                market_slippage_atoms: 3,
                ..F0Config::default()
            },
        )
        .unwrap();
        assert!(fill.triggered);
        assert_eq!(fill.fill_price, Some(PriceAtoms::new(113)));
    }

    #[test]
    fn stop_slippage_error_does_not_activate_order() {
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            Side::Buy,
            OrderKind::StopMarket {
                stop_price: PriceAtoms::new(105),
            },
            1,
        );
        assert_eq!(
            execute_bar(
                &mut orders,
                id,
                bar(2, 110, 115, 108, 112),
                F0Config {
                    intrabar_policy: IntrabarPolicy::Pessimistic,
                    market_slippage_atoms: u64::MAX,
                },
            ),
            Err(F0Error::PriceArithmetic)
        );
        assert_eq!(orders.get(id).unwrap().status, OrderStatus::Dormant);
    }

    #[test]
    fn pessimistic_ambiguous_stop_limit_triggers_but_does_not_same_bar_fill() {
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            Side::Buy,
            OrderKind::StopLimit {
                stop_price: PriceAtoms::new(105),
                limit_price: PriceAtoms::new(100),
            },
            1,
        );
        let outcome = execute_bar(
            &mut orders,
            id,
            bar(2, 102, 110, 95, 104),
            F0Config::default(),
        )
        .unwrap();
        assert!(outcome.triggered);
        assert_eq!(outcome.filled, QtyAtoms::new(0));
        assert_eq!(outcome.status, OrderStatus::Working);
        assert_eq!(
            outcome.uncertainty,
            vec![UncertaintyFlag::IntrabarAmbiguous]
        );
    }

    #[test]
    fn optimistic_ambiguous_stop_limit_fills_and_is_flagged() {
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            Side::Buy,
            OrderKind::StopLimit {
                stop_price: PriceAtoms::new(105),
                limit_price: PriceAtoms::new(100),
            },
            1,
        );
        let outcome = execute_bar(
            &mut orders,
            id,
            bar(2, 102, 110, 95, 104),
            F0Config {
                intrabar_policy: IntrabarPolicy::Optimistic,
                market_slippage_atoms: 0,
            },
        )
        .unwrap();
        assert_eq!(outcome.filled, QtyAtoms::new(10));
        assert_eq!(outcome.fill_price, Some(PriceAtoms::new(100)));
        assert_eq!(
            outcome.uncertainty,
            vec![UncertaintyFlag::IntrabarAmbiguous]
        );
    }

    #[test]
    fn seeded_ambiguity_is_reproducible() {
        let policy = IntrabarPolicy::Seeded { seed: 42 };
        let first = resolve_ambiguous(policy, 100, 9);
        assert_eq!(first, resolve_ambiguous(policy, 100, 9));
        assert_eq!(first, resolve_ambiguous(policy, 100, 9));
    }

    #[test]
    fn invalid_bar_fails_before_order_mutation() {
        let mut orders = OrderState::new();
        let id = submit(&mut orders, Side::Buy, OrderKind::Market, 1);
        let before = orders.get(id).unwrap().clone();
        assert_eq!(
            execute_bar(
                &mut orders,
                id,
                bar(2, 100, 90, 95, 98),
                F0Config::default(),
            ),
            Err(F0Error::InvalidBar)
        );
        assert_eq!(orders.get(id), Some(&before));
    }
}
