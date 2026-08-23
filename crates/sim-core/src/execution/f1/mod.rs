//! F1 deterministic execution from visible BBO quotes and later trade prints.

use crate::numeric::{PriceAtoms, QtyAtoms};
use crate::orders::{
    OrderError, OrderId, OrderKind, OrderState, OrderStatus, Side, TimeInForce, TopOfBook,
    TriggerOutcome,
};

/// One visible best-bid/best-ask observation with displayed size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BboQuote {
    /// Canonical market-event sequence for this quote.
    pub event_seq: u64,
    /// Canonical event time in nanoseconds.
    pub event_time_ns: i64,
    /// Best bid price.
    pub bid: PriceAtoms,
    /// Displayed quantity at the best bid.
    pub bid_size: QtyAtoms,
    /// Best ask price.
    pub ask: PriceAtoms,
    /// Displayed quantity at the best ask.
    pub ask_size: QtyAtoms,
}

impl BboQuote {
    /// Validates a positive, non-crossed quote.
    ///
    /// Zero displayed size is allowed because a feed can expose a price while the usable
    /// player-size cap is zero for that observation.
    ///
    /// # Errors
    /// Returns [`F1Error::InvalidQuote`] for non-positive or crossed prices.
    pub fn validate(self) -> Result<(), F1Error> {
        TopOfBook::new(self.bid, self.ask)
            .map(|_| ())
            .map_err(|_| F1Error::InvalidQuote)
    }

    fn top(self) -> Result<TopOfBook, F1Error> {
        TopOfBook::new(self.bid, self.ask).map_err(|_| F1Error::InvalidQuote)
    }
}

/// One canonical public trade print.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TradePrint {
    /// Canonical market-event sequence for this print.
    pub event_seq: u64,
    /// Canonical event time in nanoseconds.
    pub event_time_ns: i64,
    /// Published trade price.
    pub price: PriceAtoms,
    /// Published trade quantity.
    pub quantity: QtyAtoms,
}

impl TradePrint {
    /// Validates positive trade price and quantity.
    ///
    /// # Errors
    /// Returns [`F1Error::InvalidTrade`] for a non-positive price or zero quantity.
    pub fn validate(self) -> Result<(), F1Error> {
        if self.price.get() <= 0 || self.quantity.get() == 0 {
            return Err(F1Error::InvalidTrade);
        }
        Ok(())
    }
}

/// Rounding rule for a half-tick BBO midpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MidpointRounding {
    /// Round an odd-spread midpoint toward the bid.
    TowardBid,
    /// Round an odd-spread midpoint toward the ask.
    TowardAsk,
}

/// Visible price shortcut derived from one BBO observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuoteReference {
    /// Best bid.
    Bid,
    /// Best ask.
    Ask,
    /// Midpoint with an explicit half-tick rounding rule.
    Midpoint(MidpointRounding),
}

/// Liquidity role assigned by the F1 approximation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum F1LiquidityRole {
    /// Order consumed currently displayed BBO liquidity.
    Taker,
    /// Resting order was approximated as filled from a later public trade print.
    Maker,
}

/// Explicit F1 model uncertainty.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum F1Uncertainty {
    /// Public prints cannot reveal the exact private queue position of the player order.
    MakerQueueApproximation,
}

/// F1 size and freshness limits committed with simulator rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct F1Config {
    /// Maximum accepted quote age relative to logical replay time.
    pub max_quote_age_ns: u64,
    /// Maximum accepted trade-print age relative to logical replay time.
    pub max_trade_age_ns: u64,
    /// Optional additional cap on one taker fill, after displayed BBO size.
    pub max_taker_fill: Option<QtyAtoms>,
    /// Optional cap on one maker-approximation fill.
    pub max_maker_fill: Option<QtyAtoms>,
}

impl Default for F1Config {
    fn default() -> Self {
        Self {
            max_quote_age_ns: u64::MAX,
            max_trade_age_ns: u64::MAX,
            max_taker_fill: None,
            max_maker_fill: None,
        }
    }
}

/// Queue facts visible to an F1 maker heuristic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueInput {
    /// Player quantity still resting before this print.
    pub order_remaining: QtyAtoms,
    /// Quantity published by the eligible later trade print.
    pub trade_quantity: QtyAtoms,
    /// Displayed/estimated quantity believed to be ahead of the player.
    pub displayed_ahead: QtyAtoms,
}

/// Deterministic interface for translating a public print into approximated maker fill size.
pub trait QueueHeuristic {
    /// Returns maker-fillable quantity for the supplied visible queue facts.
    ///
    /// The execution engine clamps the result to the print quantity, player remaining
    /// quantity, and configured size cap, so heuristic implementations cannot overfill.
    fn fillable(&self, input: QueueInput) -> QtyAtoms;
}

/// Conservative queue rule: the print consumes displayed quantity ahead before the player.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DisplayedAheadQueue;

impl QueueHeuristic for DisplayedAheadQueue {
    fn fillable(&self, input: QueueInput) -> QtyAtoms {
        QtyAtoms::new(
            input
                .trade_quantity
                .get()
                .saturating_sub(input.displayed_ahead.get())
                .min(input.order_remaining.get()),
        )
    }
}

/// Deterministic result of offering one visible F1 event to an order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct F1Outcome {
    /// Order evaluated.
    pub order_id: OrderId,
    /// Quantity filled by this event.
    pub filled: QtyAtoms,
    /// Execution price when a fill occurred.
    pub fill_price: Option<PriceAtoms>,
    /// Approximate liquidity role when a fill occurred.
    pub liquidity_role: Option<F1LiquidityRole>,
    /// Whether a dormant stop activated during quote processing.
    pub triggered: bool,
    /// Final authoritative order status.
    pub status: OrderStatus,
    /// Explicit approximation flags.
    pub uncertainty: Vec<F1Uncertainty>,
}

/// Stable fail-closed F1 errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum F1Error {
    /// BBO prices are invalid or crossed.
    InvalidQuote,
    /// Trade price/size is invalid.
    InvalidTrade,
    /// Market data sequence is beyond the currently visible replay frontier.
    FutureMarketData,
    /// Quote is older than the committed staleness limit.
    StaleQuote,
    /// Trade print is older than the committed staleness limit.
    StaleTrade,
    /// Midpoint arithmetic could not fit a price atom.
    PriceArithmetic,
    /// Authoritative order transition failed.
    Order(OrderError),
}

impl core::fmt::Display for F1Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidQuote => formatter.write_str("invalid F1 BBO quote"),
            Self::InvalidTrade => formatter.write_str("invalid F1 trade print"),
            Self::FutureMarketData => formatter.write_str("F1 market data is beyond visible frontier"),
            Self::StaleQuote => formatter.write_str("F1 BBO quote exceeds staleness limit"),
            Self::StaleTrade => formatter.write_str("F1 trade print exceeds staleness limit"),
            Self::PriceArithmetic => formatter.write_str("F1 price arithmetic failed"),
            Self::Order(error) => write!(formatter, "F1 order transition failed: {error}"),
        }
    }
}

impl std::error::Error for F1Error {}

impl From<OrderError> for F1Error {
    fn from(value: OrderError) -> Self {
        Self::Order(value)
    }
}

/// Resolves a visible bid, ask, or explicitly rounded midpoint without floating point.
///
/// # Errors
/// Returns [`F1Error::InvalidQuote`] or [`F1Error::PriceArithmetic`].
pub fn quote_reference_price(
    quote: BboQuote,
    reference: QuoteReference,
) -> Result<PriceAtoms, F1Error> {
    quote.validate()?;
    match reference {
        QuoteReference::Bid => Ok(quote.bid),
        QuoteReference::Ask => Ok(quote.ask),
        QuoteReference::Midpoint(rounding) => {
            let sum = i128::from(quote.bid.get()) + i128::from(quote.ask.get());
            let floor = sum / 2;
            let rounded = if sum % 2 == 0 || rounding == MidpointRounding::TowardBid {
                floor
            } else {
                floor.checked_add(1).ok_or(F1Error::PriceArithmetic)?
            };
            let value = i64::try_from(rounded).map_err(|_| F1Error::PriceArithmetic)?;
            Ok(PriceAtoms::new(value))
        }
    }
}

/// Executes current visible BBO liquidity against one market/marketable order.
///
/// A currently visible quote may predate submission and still price an immediate taker order,
/// provided it passes the staleness check. Dormant stops are stricter: they only activate on a
/// quote whose event sequence is later than submission, so a stop can never trigger from a
/// price observation that was already visible when the order was entered. Non-marketable
/// IOC/FOK limits are atomically cancelled with no fill.
///
/// # Errors
/// Returns [`F1Error`] before any mutation for invalid/future/stale market data, and delegates
/// atomic lifecycle failures to [`OrderState`].
#[allow(clippy::too_many_arguments)]
pub fn execute_on_quote(
    orders: &mut OrderState,
    order_id: OrderId,
    quote: BboQuote,
    visible_event_seq: u64,
    logical_time_ns: i64,
    config: F1Config,
) -> Result<F1Outcome, F1Error> {
    quote.validate()?;
    validate_visibility_and_age(
        quote.event_seq,
        quote.event_time_ns,
        visible_event_seq,
        logical_time_ns,
        config.max_quote_age_ns,
        F1Error::StaleQuote,
    )?;

    let before = orders
        .get(order_id)
        .ok_or(OrderError::UnknownOrder)?
        .clone();
    if before.status.is_terminal() {
        return Ok(no_fill(&before, false, Vec::new()));
    }

    let mut triggered = false;
    if before.status == OrderStatus::Dormant {
        if quote.event_seq <= before.submitted_at_event_seq {
            return Ok(no_fill(&before, false, Vec::new()));
        }
        let trigger_price = match before.side {
            Side::Buy => quote.ask,
            Side::Sell => quote.bid,
        };
        match orders.trigger_stop(order_id, trigger_price, Some(quote.top()?))? {
            TriggerOutcome::NotTriggered => return Ok(no_fill(&before, false, Vec::new())),
            TriggerOutcome::Rejected => {
                let order = orders.get(order_id).ok_or(OrderError::UnknownOrder)?;
                return Ok(no_fill(order, true, Vec::new()));
            }
            TriggerOutcome::Activated => triggered = true,
        }
    }

    let active = orders
        .get(order_id)
        .ok_or(OrderError::UnknownOrder)?
        .clone();
    if !active.is_executable() {
        return Ok(no_fill(&active, triggered, Vec::new()));
    }

    let taker = match active.kind {
        OrderKind::Market => Some(taker_side(active.side, quote)),
        OrderKind::Limit { limit_price } if limit_crosses(active.side, limit_price, quote) => {
            Some(taker_side(active.side, quote))
        }
        OrderKind::Limit { .. } => None,
        OrderKind::StopMarket { .. } | OrderKind::StopLimit { .. } => {
            return Err(OrderError::InvalidState.into());
        }
    };

    let Some((fill_price, displayed)) = taker else {
        if matches!(active.time_in_force, TimeInForce::Ioc | TimeInForce::Fok) {
            let immediate = orders.execute_immediate(order_id, QtyAtoms::new(0))?;
            return Ok(F1Outcome {
                order_id,
                filled: immediate.filled,
                fill_price: None,
                liquidity_role: None,
                triggered,
                status: immediate.status,
                uncertainty: Vec::new(),
            });
        }
        return Ok(no_fill(&active, triggered, Vec::new()));
    };

    let available = cap_quantity(displayed, config.max_taker_fill);
    let filled = apply_available(orders, order_id, active.time_in_force, available)?;
    let status = orders.get(order_id).ok_or(OrderError::UnknownOrder)?.status;
    Ok(F1Outcome {
        order_id,
        filled,
        fill_price: (filled.get() > 0).then_some(fill_price),
        liquidity_role: (filled.get() > 0).then_some(F1LiquidityRole::Taker),
        triggered,
        status,
        uncertainty: Vec::new(),
    })
}

/// Approximates a resting GTC limit fill from one later visible public trade print.
///
/// `eligible_after_event_seq` is the caller-owned resting frontier. It should be advanced on
/// activation/replacement so a print from before the current resting revision cannot fill the
/// order. The function also requires a print later than original submission and rejects any
/// print beyond the visible replay frontier, preventing future-print leakage by construction.
///
/// # Errors
/// Returns [`F1Error`] before mutation for invalid/future/stale prints or an invalid order state.
#[allow(clippy::too_many_arguments)]
pub fn execute_resting_on_trade<H: QueueHeuristic>(
    orders: &mut OrderState,
    order_id: OrderId,
    trade: TradePrint,
    visible_event_seq: u64,
    logical_time_ns: i64,
    eligible_after_event_seq: u64,
    displayed_ahead: QtyAtoms,
    heuristic: &H,
    config: F1Config,
) -> Result<F1Outcome, F1Error> {
    trade.validate()?;
    validate_visibility_and_age(
        trade.event_seq,
        trade.event_time_ns,
        visible_event_seq,
        logical_time_ns,
        config.max_trade_age_ns,
        F1Error::StaleTrade,
    )?;

    let order = orders
        .get(order_id)
        .ok_or(OrderError::UnknownOrder)?
        .clone();
    if order.status.is_terminal() || !order.is_executable() {
        return Ok(no_fill(&order, false, Vec::new()));
    }
    if order.time_in_force != TimeInForce::Gtc {
        return Err(OrderError::ImmediateOrderRequiresAtomicExecution.into());
    }
    let frontier = order.submitted_at_event_seq.max(eligible_after_event_seq);
    if trade.event_seq <= frontier {
        return Ok(no_fill(&order, false, Vec::new()));
    }

    let limit_price = match order.kind {
        OrderKind::Limit { limit_price } => limit_price,
        OrderKind::Market => return Ok(no_fill(&order, false, Vec::new())),
        OrderKind::StopMarket { .. } | OrderKind::StopLimit { .. } => {
            return Err(OrderError::InvalidState.into());
        }
    };
    if !trade_reaches_resting_limit(order.side, limit_price, trade.price) {
        return Ok(no_fill(&order, false, Vec::new()));
    }

    let proposed = heuristic.fillable(QueueInput {
        order_remaining: order.remaining(),
        trade_quantity: trade.quantity,
        displayed_ahead,
    });
    let bounded_by_print = QtyAtoms::new(
        proposed
            .get()
            .min(trade.quantity.get())
            .min(order.remaining().get()),
    );
    let available = cap_quantity(bounded_by_print, config.max_maker_fill);
    if available.get() == 0 {
        return Ok(no_fill(
            &order,
            false,
            vec![F1Uncertainty::MakerQueueApproximation],
        ));
    }

    orders.record_fill(order_id, available)?;
    let status = orders.get(order_id).ok_or(OrderError::UnknownOrder)?.status;
    Ok(F1Outcome {
        order_id,
        filled: available,
        fill_price: Some(limit_price),
        liquidity_role: Some(F1LiquidityRole::Maker),
        triggered: false,
        status,
        uncertainty: vec![F1Uncertainty::MakerQueueApproximation],
    })
}

fn validate_visibility_and_age(
    event_seq: u64,
    event_time_ns: i64,
    visible_event_seq: u64,
    logical_time_ns: i64,
    max_age_ns: u64,
    stale_error: F1Error,
) -> Result<(), F1Error> {
    if event_seq > visible_event_seq || event_time_ns > logical_time_ns {
        return Err(F1Error::FutureMarketData);
    }
    let age = i128::from(logical_time_ns) - i128::from(event_time_ns);
    let age = u128::try_from(age).map_err(|_| F1Error::FutureMarketData)?;
    if age > u128::from(max_age_ns) {
        return Err(stale_error);
    }
    Ok(())
}

fn no_fill(
    order: &crate::orders::Order,
    triggered: bool,
    uncertainty: Vec<F1Uncertainty>,
) -> F1Outcome {
    F1Outcome {
        order_id: order.id,
        filled: QtyAtoms::new(0),
        fill_price: None,
        liquidity_role: None,
        triggered,
        status: order.status,
        uncertainty,
    }
}

fn taker_side(side: Side, quote: BboQuote) -> (PriceAtoms, QtyAtoms) {
    match side {
        Side::Buy => (quote.ask, quote.ask_size),
        Side::Sell => (quote.bid, quote.bid_size),
    }
}

fn limit_crosses(side: Side, limit: PriceAtoms, quote: BboQuote) -> bool {
    match side {
        Side::Buy => limit >= quote.ask,
        Side::Sell => limit <= quote.bid,
    }
}

fn trade_reaches_resting_limit(side: Side, limit: PriceAtoms, trade: PriceAtoms) -> bool {
    match side {
        Side::Buy => trade <= limit,
        Side::Sell => trade >= limit,
    }
}

fn cap_quantity(quantity: QtyAtoms, cap: Option<QtyAtoms>) -> QtyAtoms {
    match cap {
        Some(cap) => QtyAtoms::new(quantity.get().min(cap.get())),
        None => quantity,
    }
}

fn apply_available(
    orders: &mut OrderState,
    order_id: OrderId,
    time_in_force: TimeInForce,
    available: QtyAtoms,
) -> Result<QtyAtoms, OrderError> {
    let remaining = orders
        .get(order_id)
        .ok_or(OrderError::UnknownOrder)?
        .remaining();
    match time_in_force {
        TimeInForce::Gtc => {
            let filled = QtyAtoms::new(available.get().min(remaining.get()));
            if filled.get() > 0 {
                orders.record_fill(order_id, filled)?;
            }
            Ok(filled)
        }
        TimeInForce::Ioc | TimeInForce::Fok => {
            Ok(orders.execute_immediate(order_id, available)?.filled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::{NewOrder, OrderKind, OrderStatus};

    fn quote(seq: u64, time: i64) -> BboQuote {
        BboQuote {
            event_seq: seq,
            event_time_ns: time,
            bid: PriceAtoms::new(100),
            bid_size: QtyAtoms::new(6),
            ask: PriceAtoms::new(103),
            ask_size: QtyAtoms::new(4),
        }
    }

    fn submit(
        orders: &mut OrderState,
        kind: OrderKind,
        tif: TimeInForce,
        side: Side,
        quantity: u64,
        submitted_at_event_seq: u64,
    ) -> OrderId {
        orders
            .submit(
                NewOrder {
                    client_order_id: format!("client-{submitted_at_event_seq}-{quantity}"),
                    instrument_id: "SYNTH".into(),
                    side,
                    quantity: QtyAtoms::new(quantity),
                    kind,
                    time_in_force: tif,
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
    fn bid_ask_and_midpoint_use_explicit_integer_rounding() {
        let quote = quote(1, 10);
        assert_eq!(
            quote_reference_price(quote, QuoteReference::Bid).unwrap(),
            PriceAtoms::new(100)
        );
        assert_eq!(
            quote_reference_price(quote, QuoteReference::Ask).unwrap(),
            PriceAtoms::new(103)
        );
        assert_eq!(
            quote_reference_price(
                quote,
                QuoteReference::Midpoint(MidpointRounding::TowardBid)
            )
            .unwrap(),
            PriceAtoms::new(101)
        );
        assert_eq!(
            quote_reference_price(
                quote,
                QuoteReference::Midpoint(MidpointRounding::TowardAsk)
            )
            .unwrap(),
            PriceAtoms::new(102)
        );
    }

    #[test]
    fn ioc_market_partially_takes_ask_and_cancels_remainder() {
        let mut orders = OrderState::new();
        let id = submit(&mut orders, OrderKind::Market, TimeInForce::Ioc, Side::Buy, 10, 5);
        let outcome = execute_on_quote(&mut orders, id, quote(5, 100), 5, 100, F1Config::default())
            .unwrap();
        assert_eq!(outcome.filled, QtyAtoms::new(4));
        assert_eq!(outcome.fill_price, Some(PriceAtoms::new(103)));
        assert_eq!(outcome.liquidity_role, Some(F1LiquidityRole::Taker));
        assert_eq!(outcome.status, OrderStatus::Cancelled);
    }

    #[test]
    fn marketable_limit_gets_quote_price_not_limit_price() {
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            OrderKind::Limit {
                limit_price: PriceAtoms::new(110),
            },
            TimeInForce::Gtc,
            Side::Buy,
            2,
            5,
        );
        let outcome = execute_on_quote(&mut orders, id, quote(5, 100), 5, 100, F1Config::default())
            .unwrap();
        assert_eq!(outcome.filled, QtyAtoms::new(2));
        assert_eq!(outcome.fill_price, Some(PriceAtoms::new(103)));
        assert_eq!(outcome.status, OrderStatus::Filled);
    }

    #[test]
    fn configured_taker_size_cap_is_applied_after_displayed_size() {
        let mut orders = OrderState::new();
        let id = submit(&mut orders, OrderKind::Market, TimeInForce::Gtc, Side::Sell, 10, 1);
        let config = F1Config {
            max_taker_fill: Some(QtyAtoms::new(2)),
            ..F1Config::default()
        };
        let outcome = execute_on_quote(&mut orders, id, quote(1, 100), 1, 100, config).unwrap();
        assert_eq!(outcome.filled, QtyAtoms::new(2));
        assert_eq!(outcome.fill_price, Some(PriceAtoms::new(100)));
        assert_eq!(outcome.status, OrderStatus::PartiallyFilled);
    }

    #[test]
    fn stale_quote_cannot_trigger_a_stop() {
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            OrderKind::StopMarket {
                stop_price: PriceAtoms::new(102),
            },
            TimeInForce::Gtc,
            Side::Buy,
            1,
            1,
        );
        let config = F1Config {
            max_quote_age_ns: 5,
            ..F1Config::default()
        };
        assert_eq!(
            execute_on_quote(&mut orders, id, quote(2, 100), 2, 106, config),
            Err(F1Error::StaleQuote)
        );
        assert_eq!(orders.get(id).unwrap().status, OrderStatus::Dormant);
    }

    #[test]
    fn stop_uses_only_a_later_quote_then_takes_visible_ask() {
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            OrderKind::StopMarket {
                stop_price: PriceAtoms::new(102),
            },
            TimeInForce::Gtc,
            Side::Buy,
            1,
            7,
        );
        let same = execute_on_quote(&mut orders, id, quote(7, 100), 7, 100, F1Config::default())
            .unwrap();
        assert!(!same.triggered);
        assert_eq!(same.filled, QtyAtoms::new(0));
        let later = execute_on_quote(&mut orders, id, quote(8, 101), 8, 101, F1Config::default())
            .unwrap();
        assert!(later.triggered);
        assert_eq!(later.filled, QtyAtoms::new(1));
        assert_eq!(later.fill_price, Some(PriceAtoms::new(103)));
    }

    #[test]
    fn resting_limit_requires_later_visible_print_and_never_future_print() {
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            OrderKind::Limit {
                limit_price: PriceAtoms::new(100),
            },
            TimeInForce::Gtc,
            Side::Buy,
            5,
            10,
        );
        let same = TradePrint {
            event_seq: 10,
            event_time_ns: 100,
            price: PriceAtoms::new(100),
            quantity: QtyAtoms::new(5),
        };
        let same_outcome = execute_resting_on_trade(
            &mut orders,
            id,
            same,
            10,
            100,
            10,
            QtyAtoms::new(0),
            &DisplayedAheadQueue,
            F1Config::default(),
        )
        .unwrap();
        assert_eq!(same_outcome.filled, QtyAtoms::new(0));

        let future = TradePrint {
            event_seq: 11,
            event_time_ns: 101,
            ..same
        };
        let before = orders.clone();
        assert_eq!(
            execute_resting_on_trade(
                &mut orders,
                id,
                future,
                10,
                101,
                10,
                QtyAtoms::new(0),
                &DisplayedAheadQueue,
                F1Config::default(),
            ),
            Err(F1Error::FutureMarketData)
        );
        assert_eq!(orders, before);
    }

    #[test]
    fn displayed_queue_ahead_is_consumed_before_maker_fill() {
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            OrderKind::Limit {
                limit_price: PriceAtoms::new(100),
            },
            TimeInForce::Gtc,
            Side::Buy,
            5,
            1,
        );
        let trade = TradePrint {
            event_seq: 2,
            event_time_ns: 20,
            price: PriceAtoms::new(100),
            quantity: QtyAtoms::new(7),
        };
        let outcome = execute_resting_on_trade(
            &mut orders,
            id,
            trade,
            2,
            20,
            1,
            QtyAtoms::new(5),
            &DisplayedAheadQueue,
            F1Config::default(),
        )
        .unwrap();
        assert_eq!(outcome.filled, QtyAtoms::new(2));
        assert_eq!(outcome.fill_price, Some(PriceAtoms::new(100)));
        assert_eq!(outcome.liquidity_role, Some(F1LiquidityRole::Maker));
        assert_eq!(
            outcome.uncertainty,
            vec![F1Uncertainty::MakerQueueApproximation]
        );
    }
}
