//! F2 deterministic L2 reconstruction, depth sweeps, and displayed queue modeling.

use std::collections::BTreeMap;

use crate::numeric::{PriceAtoms, QtyAtoms};
use crate::orders::{
    OrderError, OrderId, OrderKind, OrderState, OrderStatus, Side, TimeInForce, TopOfBook,
    TriggerOutcome,
};

/// Side of one visible L2 book level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BookSide {
    /// Bid-side displayed liquidity.
    Bid,
    /// Ask-side displayed liquidity.
    Ask,
}

/// One absolute visible depth level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DepthLevel {
    /// Positive price atom.
    pub price: PriceAtoms,
    /// Positive displayed quantity.
    pub quantity: QtyAtoms,
}

/// Complete L2 snapshot that can establish or recover book continuity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct L2Snapshot {
    /// Feed sequence of the complete snapshot.
    pub sequence: u64,
    /// Bid levels. Input order is irrelevant; duplicate prices are rejected.
    pub bids: Vec<DepthLevel>,
    /// Ask levels. Input order is irrelevant; duplicate prices are rejected.
    pub asks: Vec<DepthLevel>,
}

/// Absolute-size L2 update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct L2Delta {
    /// Sequence this delta claims to follow.
    pub previous_sequence: u64,
    /// New sequence after applying this delta.
    pub sequence: u64,
    /// Side being updated.
    pub side: BookSide,
    /// Price level being updated.
    pub price: PriceAtoms,
    /// New absolute displayed quantity. Zero deletes the level.
    pub quantity: QtyAtoms,
}

/// One price/quantity component of a multi-level sweep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LevelFill {
    /// Consumed book price.
    pub price: PriceAtoms,
    /// Quantity consumed at this level.
    pub quantity: QtyAtoms,
}

/// Hard execution caps applied to one F2 sweep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SweepConfig {
    /// Maximum number of distinct price levels one command may consume.
    pub max_levels: usize,
    /// Optional hard quantity cap across all levels.
    pub max_quantity: Option<QtyAtoms>,
}

impl Default for SweepConfig {
    fn default() -> Self {
        Self {
            max_levels: usize::MAX,
            max_quantity: None,
        }
    }
}

/// Deterministic result of a taker sweep.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct F2Outcome {
    /// Order evaluated.
    pub order_id: OrderId,
    /// Total quantity filled.
    pub filled: QtyAtoms,
    /// Per-level fills in execution order.
    pub levels: Vec<LevelFill>,
    /// Whether a dormant stop activated from this book frontier.
    pub triggered: bool,
    /// Final authoritative order status.
    pub status: OrderStatus,
}

/// Allocation rule for displayed cancellations relative to a player's queue position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelAllocationPolicy {
    /// Assume cancellations occur behind the player until impossible.
    Conservative,
    /// Allocate cancellations proportionally to displayed quantity ahead.
    ProRata,
    /// Assume cancellations remove quantity ahead of the player first.
    Optimistic,
}

/// Displayed queue-ahead state for one resting order at one price.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueuePosition {
    ahead: QtyAtoms,
}

impl QueuePosition {
    /// Joins behind the currently displayed quantity at the order's price.
    #[must_use]
    pub const fn join(displayed: QtyAtoms) -> Self {
        Self { ahead: displayed }
    }

    /// Returns displayed quantity currently modeled ahead of the player.
    #[must_use]
    pub const fn ahead(self) -> QtyAtoms {
        self.ahead
    }

    /// Applies displayed cancellation under the committed allocation policy.
    ///
    /// `displayed_before` is the full displayed quantity at the level immediately before the
    /// cancellation. It must be at least the modeled quantity ahead.
    ///
    /// # Errors
    /// Returns [`F2Error::InvalidQueue`] for inconsistent displayed state or checked overflow.
    pub fn apply_cancel(
        &mut self,
        cancelled: QtyAtoms,
        displayed_before: QtyAtoms,
        policy: CancelAllocationPolicy,
    ) -> Result<QtyAtoms, F2Error> {
        if self.ahead > displayed_before || cancelled > displayed_before {
            return Err(F2Error::InvalidQueue);
        }
        let removed_ahead = match policy {
            CancelAllocationPolicy::Conservative => QtyAtoms::new(0),
            CancelAllocationPolicy::Optimistic => {
                QtyAtoms::new(cancelled.get().min(self.ahead.get()))
            }
            CancelAllocationPolicy::ProRata => {
                if displayed_before.get() == 0 || self.ahead.get() == 0 {
                    QtyAtoms::new(0)
                } else {
                    let numerator = u128::from(cancelled.get())
                        .checked_mul(u128::from(self.ahead.get()))
                        .ok_or(F2Error::QuantityArithmetic)?;
                    let allocated = numerator / u128::from(displayed_before.get());
                    let allocated = u64::try_from(allocated)
                        .map_err(|_| F2Error::QuantityArithmetic)?
                        .min(self.ahead.get());
                    QtyAtoms::new(allocated)
                }
            }
        };
        self.ahead = QtyAtoms::new(self.ahead.get() - removed_ahead.get());
        Ok(removed_ahead)
    }

    /// Applies a trade at this level, consuming queue ahead before returning player-fillable size.
    #[must_use]
    pub fn apply_trade(&mut self, traded: QtyAtoms) -> QtyAtoms {
        let consumed_ahead = traded.get().min(self.ahead.get());
        self.ahead = QtyAtoms::new(self.ahead.get() - consumed_ahead);
        QtyAtoms::new(traded.get() - consumed_ahead)
    }
}

/// Stable F2 reconstruction/execution failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum F2Error {
    /// Snapshot or delta contains invalid price/quantity/crossed depth.
    InvalidBook,
    /// Incremental sequence continuity was lost; a new snapshot is required.
    SequenceGap,
    /// Book is disabled until a complete valid snapshot is applied.
    BookDisabled,
    /// Sweep config cannot execute any level.
    InvalidSweepConfig,
    /// Displayed queue facts are internally inconsistent.
    InvalidQueue,
    /// Checked quantity arithmetic failed.
    QuantityArithmetic,
    /// Authoritative order transition failed.
    Order(OrderError),
}

impl core::fmt::Display for F2Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidBook => formatter.write_str("invalid F2 depth book"),
            Self::SequenceGap => formatter.write_str("F2 depth sequence gap requires snapshot"),
            Self::BookDisabled => formatter.write_str("F2 depth book is disabled until snapshot"),
            Self::InvalidSweepConfig => formatter.write_str("invalid F2 sweep configuration"),
            Self::InvalidQueue => formatter.write_str("invalid F2 displayed queue state"),
            Self::QuantityArithmetic => formatter.write_str("F2 quantity arithmetic failed"),
            Self::Order(error) => write!(formatter, "F2 order transition failed: {error}"),
        }
    }
}

impl std::error::Error for F2Error {}

impl From<OrderError> for F2Error {
    fn from(value: OrderError) -> Self {
        Self::Order(value)
    }
}

/// Sequence-aware reconstructed L2 book.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct L2Book {
    sequence: Option<u64>,
    enabled: bool,
    bids: BTreeMap<i64, u64>,
    asks: BTreeMap<i64, u64>,
}

impl L2Book {
    /// Creates an empty disabled book. A valid snapshot is required before use.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sequence: None,
            enabled: false,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        }
    }

    /// Returns whether incremental reconstruction is currently trustworthy.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns the currently reconstructed feed sequence.
    #[must_use]
    pub const fn sequence(&self) -> Option<u64> {
        self.sequence
    }

    /// Returns best visible bid while reconstruction is enabled.
    #[must_use]
    pub fn best_bid(&self) -> Option<DepthLevel> {
        if !self.enabled {
            return None;
        }
        self.bids.iter().next_back().map(|(&price, &quantity)| DepthLevel {
            price: PriceAtoms::new(price),
            quantity: QtyAtoms::new(quantity),
        })
    }

    /// Returns best visible ask while reconstruction is enabled.
    #[must_use]
    pub fn best_ask(&self) -> Option<DepthLevel> {
        if !self.enabled {
            return None;
        }
        self.asks.iter().next().map(|(&price, &quantity)| DepthLevel {
            price: PriceAtoms::new(price),
            quantity: QtyAtoms::new(quantity),
        })
    }

    /// Returns displayed quantity at one exact price.
    #[must_use]
    pub fn quantity_at(&self, side: BookSide, price: PriceAtoms) -> QtyAtoms {
        let map = match side {
            BookSide::Bid => &self.bids,
            BookSide::Ask => &self.asks,
        };
        QtyAtoms::new(map.get(&price.get()).copied().unwrap_or(0))
    }

    /// Returns total visible quantity on one side using checked `u128` accumulation.
    ///
    /// # Errors
    /// Returns [`F2Error::QuantityArithmetic`] on an impossible accumulator overflow.
    pub fn total_quantity(&self, side: BookSide) -> Result<u128, F2Error> {
        let map = match side {
            BookSide::Bid => &self.bids,
            BookSide::Ask => &self.asks,
        };
        map.values().try_fold(0_u128, |total, quantity| {
            total
                .checked_add(u128::from(*quantity))
                .ok_or(F2Error::QuantityArithmetic)
        })
    }

    /// Atomically installs a complete snapshot and re-enables a disabled book.
    ///
    /// # Errors
    /// Returns [`F2Error::InvalidBook`] without changing the current book if the snapshot is
    /// malformed, contains duplicate/non-positive levels, or is crossed.
    pub fn apply_snapshot(&mut self, snapshot: L2Snapshot) -> Result<(), F2Error> {
        let bids = build_side(snapshot.bids)?;
        let asks = build_side(snapshot.asks)?;
        validate_uncrossed(&bids, &asks)?;
        self.sequence = Some(snapshot.sequence);
        self.enabled = true;
        self.bids = bids;
        self.asks = asks;
        Ok(())
    }

    /// Applies one absolute delta only when feed sequence continuity is exact.
    ///
    /// A continuity gap or invalid resulting book disables incremental use immediately. The
    /// prior depth is retained only for diagnostics; no sweep can use it until a new valid
    /// snapshot replaces it.
    ///
    /// # Errors
    /// Returns [`F2Error::SequenceGap`] or [`F2Error::InvalidBook`] and disables the book when
    /// continuity or structural validity is lost.
    pub fn apply_delta(&mut self, delta: L2Delta) -> Result<(), F2Error> {
        if !self.enabled {
            return Err(F2Error::BookDisabled);
        }
        if delta.price.get() <= 0 {
            self.enabled = false;
            return Err(F2Error::InvalidBook);
        }
        let Some(sequence) = self.sequence else {
            self.enabled = false;
            return Err(F2Error::SequenceGap);
        };
        if delta.previous_sequence != sequence || delta.sequence <= delta.previous_sequence {
            self.enabled = false;
            return Err(F2Error::SequenceGap);
        }

        let mut bids = self.bids.clone();
        let mut asks = self.asks.clone();
        let target = match delta.side {
            BookSide::Bid => &mut bids,
            BookSide::Ask => &mut asks,
        };
        if delta.quantity.get() == 0 {
            target.remove(&delta.price.get());
        } else {
            target.insert(delta.price.get(), delta.quantity.get());
        }
        if validate_uncrossed(&bids, &asks).is_err() {
            self.enabled = false;
            return Err(F2Error::InvalidBook);
        }
        self.bids = bids;
        self.asks = asks;
        self.sequence = Some(delta.sequence);
        Ok(())
    }

    fn top(&self) -> Result<TopOfBook, F2Error> {
        if !self.enabled {
            return Err(F2Error::BookDisabled);
        }
        let bid = self.best_bid().ok_or(F2Error::InvalidBook)?.price;
        let ask = self.best_ask().ok_or(F2Error::InvalidBook)?.price;
        TopOfBook::new(bid, ask).map_err(|_| F2Error::InvalidBook)
    }

    fn plan_sweep(
        &self,
        side: Side,
        requested: QtyAtoms,
        limit: Option<PriceAtoms>,
        config: SweepConfig,
    ) -> Result<Vec<LevelFill>, F2Error> {
        if !self.enabled {
            return Err(F2Error::BookDisabled);
        }
        if config.max_levels == 0 {
            return Err(F2Error::InvalidSweepConfig);
        }
        let capped = match config.max_quantity {
            Some(cap) => requested.get().min(cap.get()),
            None => requested.get(),
        };
        if capped == 0 {
            return Ok(Vec::new());
        }

        let mut remaining = capped;
        let mut fills = Vec::new();
        match side {
            Side::Buy => {
                for (&price, &quantity) in self.asks.iter().take(config.max_levels) {
                    let price = PriceAtoms::new(price);
                    if limit.is_some_and(|limit| price > limit) {
                        break;
                    }
                    let fill = quantity.min(remaining);
                    if fill > 0 {
                        fills.push(LevelFill {
                            price,
                            quantity: QtyAtoms::new(fill),
                        });
                        remaining -= fill;
                    }
                    if remaining == 0 {
                        break;
                    }
                }
            }
            Side::Sell => {
                for (&price, &quantity) in self.bids.iter().rev().take(config.max_levels) {
                    let price = PriceAtoms::new(price);
                    if limit.is_some_and(|limit| price < limit) {
                        break;
                    }
                    let fill = quantity.min(remaining);
                    if fill > 0 {
                        fills.push(LevelFill {
                            price,
                            quantity: QtyAtoms::new(fill),
                        });
                        remaining -= fill;
                    }
                    if remaining == 0 {
                        break;
                    }
                }
            }
        }
        Ok(fills)
    }

    fn consume(&mut self, side: Side, fills: &[LevelFill]) -> Result<(), F2Error> {
        let map = match side {
            Side::Buy => &mut self.asks,
            Side::Sell => &mut self.bids,
        };
        for fill in fills {
            let current = map
                .get(&fill.price.get())
                .copied()
                .ok_or(F2Error::InvalidBook)?;
            if fill.quantity.get() > current {
                return Err(F2Error::InvalidBook);
            }
            let remaining = current - fill.quantity.get();
            if remaining == 0 {
                map.remove(&fill.price.get());
            } else {
                map.insert(fill.price.get(), remaining);
            }
        }
        Ok(())
    }
}

/// Executes one market/marketable-limit order against reconstructed depth atomically with book
/// consumption. Dormant stops may activate only on a book sequence later than submission.
///
/// FOK insufficiency cancels the order without consuming depth. IOC consumes the planned
/// quantity and cancels its remainder. GTC consumes available depth and remains working if
/// partially filled. Both `OrderState` and `L2Book` are updated only after all transitions
/// succeed on clones.
///
/// # Errors
/// Returns [`F2Error`] without partially updating either authoritative state.
pub fn execute_taker(
    orders: &mut OrderState,
    book: &mut L2Book,
    order_id: OrderId,
    config: SweepConfig,
) -> Result<F2Outcome, F2Error> {
    if !book.enabled {
        return Err(F2Error::BookDisabled);
    }
    if config.max_levels == 0 {
        return Err(F2Error::InvalidSweepConfig);
    }
    let before = orders
        .get(order_id)
        .ok_or(OrderError::UnknownOrder)?
        .clone();
    if before.status.is_terminal() {
        return Ok(no_fill(&before, false));
    }

    let mut next_orders = orders.clone();
    let mut triggered = false;
    if before.status == OrderStatus::Dormant {
        let sequence = book.sequence.ok_or(F2Error::BookDisabled)?;
        if sequence <= before.submitted_at_event_seq {
            return Ok(no_fill(&before, false));
        }
        let top = book.top()?;
        let trigger_price = match before.side {
            Side::Buy => top.ask,
            Side::Sell => top.bid,
        };
        match next_orders.trigger_stop(order_id, trigger_price, Some(top))? {
            TriggerOutcome::NotTriggered => return Ok(no_fill(&before, false)),
            TriggerOutcome::Rejected => {
                let rejected = next_orders
                    .get(order_id)
                    .ok_or(OrderError::UnknownOrder)?
                    .clone();
                *orders = next_orders;
                return Ok(no_fill(&rejected, true));
            }
            TriggerOutcome::Activated => triggered = true,
        }
    }

    let active = next_orders
        .get(order_id)
        .ok_or(OrderError::UnknownOrder)?
        .clone();
    if !active.is_executable() {
        return Ok(no_fill(&active, triggered));
    }
    let limit = match active.kind {
        OrderKind::Market => None,
        OrderKind::Limit { limit_price } => Some(limit_price),
        OrderKind::StopMarket { .. } | OrderKind::StopLimit { .. } => {
            return Err(OrderError::InvalidState.into());
        }
    };
    let plan = book.plan_sweep(active.side, active.remaining(), limit, config)?;
    let available = sum_fills(&plan)?;

    let filled = match active.time_in_force {
        TimeInForce::Gtc => {
            if available.get() > 0 {
                next_orders.record_fill(order_id, available)?;
            }
            available
        }
        TimeInForce::Ioc | TimeInForce::Fok => {
            next_orders.execute_immediate(order_id, available)?.filled
        }
    };

    let consumed = prefix_for_quantity(&plan, filled)?;
    let mut next_book = book.clone();
    next_book.consume(active.side, &consumed)?;
    let status = next_orders
        .get(order_id)
        .ok_or(OrderError::UnknownOrder)?
        .status;
    *orders = next_orders;
    *book = next_book;
    Ok(F2Outcome {
        order_id,
        filled,
        levels: consumed,
        triggered,
        status,
    })
}

/// Applies one trade at a resting order's price through explicit queue-ahead state.
///
/// Only later-sequence GTC limit orders are eligible. The queue and order state are cloned and
/// committed together, so an order transition failure cannot consume queue position.
///
/// # Errors
/// Returns [`F2Error`] without partial mutation for invalid order/queue state.
#[allow(clippy::too_many_arguments)]
pub fn execute_resting_trade(
    orders: &mut OrderState,
    queue: &mut QueuePosition,
    order_id: OrderId,
    trade_event_seq: u64,
    trade_price: PriceAtoms,
    trade_quantity: QtyAtoms,
) -> Result<QtyAtoms, F2Error> {
    if trade_price.get() <= 0 || trade_quantity.get() == 0 {
        return Err(F2Error::InvalidBook);
    }
    let order = orders
        .get(order_id)
        .ok_or(OrderError::UnknownOrder)?
        .clone();
    if !order.is_executable() {
        return Ok(QtyAtoms::new(0));
    }
    if order.time_in_force != TimeInForce::Gtc {
        return Err(OrderError::ImmediateOrderRequiresAtomicExecution.into());
    }
    if trade_event_seq <= order.submitted_at_event_seq {
        return Ok(QtyAtoms::new(0));
    }
    let limit = match order.kind {
        OrderKind::Limit { limit_price } => limit_price,
        OrderKind::Market => return Ok(QtyAtoms::new(0)),
        OrderKind::StopMarket { .. } | OrderKind::StopLimit { .. } => {
            return Err(OrderError::InvalidState.into());
        }
    };
    let reached = match order.side {
        Side::Buy => trade_price <= limit,
        Side::Sell => trade_price >= limit,
    };
    if !reached {
        return Ok(QtyAtoms::new(0));
    }

    let mut next_queue = *queue;
    let fillable = next_queue.apply_trade(trade_quantity);
    let fill = QtyAtoms::new(fillable.get().min(order.remaining().get()));
    if fill.get() == 0 {
        *queue = next_queue;
        return Ok(fill);
    }
    let mut next_orders = orders.clone();
    next_orders.record_fill(order_id, fill)?;
    *orders = next_orders;
    *queue = next_queue;
    Ok(fill)
}

fn build_side(levels: Vec<DepthLevel>) -> Result<BTreeMap<i64, u64>, F2Error> {
    let mut output = BTreeMap::new();
    for level in levels {
        if level.price.get() <= 0 || level.quantity.get() == 0 {
            return Err(F2Error::InvalidBook);
        }
        if output
            .insert(level.price.get(), level.quantity.get())
            .is_some()
        {
            return Err(F2Error::InvalidBook);
        }
    }
    Ok(output)
}

fn validate_uncrossed(
    bids: &BTreeMap<i64, u64>,
    asks: &BTreeMap<i64, u64>,
) -> Result<(), F2Error> {
    if let (Some((&bid, _)), Some((&ask, _))) = (bids.iter().next_back(), asks.iter().next()) {
        if bid >= ask {
            return Err(F2Error::InvalidBook);
        }
    }
    Ok(())
}

fn sum_fills(fills: &[LevelFill]) -> Result<QtyAtoms, F2Error> {
    let total = fills.iter().try_fold(0_u64, |total, fill| {
        total
            .checked_add(fill.quantity.get())
            .ok_or(F2Error::QuantityArithmetic)
    })?;
    Ok(QtyAtoms::new(total))
}

fn prefix_for_quantity(fills: &[LevelFill], quantity: QtyAtoms) -> Result<Vec<LevelFill>, F2Error> {
    let mut remaining = quantity.get();
    let mut output = Vec::new();
    for level in fills {
        if remaining == 0 {
            break;
        }
        let fill = level.quantity.get().min(remaining);
        if fill > 0 {
            output.push(LevelFill {
                price: level.price,
                quantity: QtyAtoms::new(fill),
            });
            remaining -= fill;
        }
    }
    if remaining != 0 {
        return Err(F2Error::QuantityArithmetic);
    }
    Ok(output)
}

fn no_fill(order: &crate::orders::Order, triggered: bool) -> F2Outcome {
    F2Outcome {
        order_id: order.id,
        filled: QtyAtoms::new(0),
        levels: Vec::new(),
        triggered,
        status: order.status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::{NewOrder, OrderKind};

    fn level(price: i64, quantity: u64) -> DepthLevel {
        DepthLevel {
            price: PriceAtoms::new(price),
            quantity: QtyAtoms::new(quantity),
        }
    }

    fn snapshot(sequence: u64) -> L2Snapshot {
        L2Snapshot {
            sequence,
            bids: vec![level(99, 5), level(100, 4)],
            asks: vec![level(101, 3), level(102, 4), level(103, 5)],
        }
    }

    fn submit(
        orders: &mut OrderState,
        kind: OrderKind,
        tif: TimeInForce,
        side: Side,
        quantity: u64,
        sequence: u64,
    ) -> OrderId {
        orders
            .submit(
                NewOrder {
                    client_order_id: format!("f2-{sequence}-{quantity}"),
                    instrument_id: "SYNTH".into(),
                    side,
                    quantity: QtyAtoms::new(quantity),
                    kind,
                    time_in_force: tif,
                    reduce_only: false,
                    post_only: false,
                    marketable_only: false,
                    submitted_at_event_seq: sequence,
                },
                0,
                None,
            )
            .unwrap()
            .order_id
    }

    #[test]
    fn valid_snapshot_reconstructs_best_levels_and_totals() {
        let mut book = L2Book::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        assert!(book.is_enabled());
        assert_eq!(book.sequence(), Some(10));
        assert_eq!(book.best_bid(), Some(level(100, 4)));
        assert_eq!(book.best_ask(), Some(level(101, 3)));
        assert_eq!(book.total_quantity(BookSide::Bid).unwrap(), 9);
        assert_eq!(book.total_quantity(BookSide::Ask).unwrap(), 12);
    }

    #[test]
    fn sequence_gap_disables_deltas_until_snapshot_recovery() {
        let mut book = L2Book::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let before = book.clone();
        assert_eq!(
            book.apply_delta(L2Delta {
                previous_sequence: 9,
                sequence: 11,
                side: BookSide::Ask,
                price: PriceAtoms::new(101),
                quantity: QtyAtoms::new(2),
            }),
            Err(F2Error::SequenceGap)
        );
        assert!(!book.is_enabled());
        assert_eq!(book.asks, before.asks);
        assert_eq!(
            book.apply_delta(L2Delta {
                previous_sequence: 10,
                sequence: 11,
                side: BookSide::Ask,
                price: PriceAtoms::new(101),
                quantity: QtyAtoms::new(2),
            }),
            Err(F2Error::BookDisabled)
        );
        book.apply_snapshot(snapshot(20)).unwrap();
        assert!(book.is_enabled());
        assert_eq!(book.sequence(), Some(20));
    }

    #[test]
    fn crossed_delta_disables_without_applying_depth() {
        let mut book = L2Book::new();
        book.apply_snapshot(snapshot(1)).unwrap();
        assert_eq!(
            book.apply_delta(L2Delta {
                previous_sequence: 1,
                sequence: 2,
                side: BookSide::Bid,
                price: PriceAtoms::new(101),
                quantity: QtyAtoms::new(9),
            }),
            Err(F2Error::InvalidBook)
        );
        assert!(!book.is_enabled());
        assert_eq!(book.quantity_at(BookSide::Bid, PriceAtoms::new(101)), QtyAtoms::new(0));
    }

    #[test]
    fn market_sweep_consumes_multiple_levels_and_conserves_depth() {
        let mut book = L2Book::new();
        book.apply_snapshot(snapshot(2)).unwrap();
        let before = book.total_quantity(BookSide::Ask).unwrap();
        let mut orders = OrderState::new();
        let id = submit(&mut orders, OrderKind::Market, TimeInForce::Gtc, Side::Buy, 6, 1);
        let outcome = execute_taker(&mut orders, &mut book, id, SweepConfig::default()).unwrap();
        assert_eq!(
            outcome.levels,
            vec![level_fill(101, 3), level_fill(102, 3)]
        );
        assert_eq!(outcome.filled, QtyAtoms::new(6));
        assert_eq!(outcome.status, OrderStatus::Filled);
        assert_eq!(book.quantity_at(BookSide::Ask, PriceAtoms::new(101)), QtyAtoms::new(0));
        assert_eq!(book.quantity_at(BookSide::Ask, PriceAtoms::new(102)), QtyAtoms::new(1));
        let after = book.total_quantity(BookSide::Ask).unwrap();
        assert_eq!(before - after, u128::from(outcome.filled.get()));
    }

    #[test]
    fn limit_sweep_never_consumes_worse_price() {
        let mut book = L2Book::new();
        book.apply_snapshot(snapshot(2)).unwrap();
        let mut orders = OrderState::new();
        let id = submit(
            &mut orders,
            OrderKind::Limit {
                limit_price: PriceAtoms::new(101),
            },
            TimeInForce::Gtc,
            Side::Buy,
            6,
            1,
        );
        let outcome = execute_taker(&mut orders, &mut book, id, SweepConfig::default()).unwrap();
        assert_eq!(outcome.levels, vec![level_fill(101, 3)]);
        assert_eq!(outcome.filled, QtyAtoms::new(3));
        assert_eq!(book.quantity_at(BookSide::Ask, PriceAtoms::new(102)), QtyAtoms::new(4));
    }

    #[test]
    fn insufficient_fok_cancels_without_consuming_depth() {
        let mut book = L2Book::new();
        book.apply_snapshot(snapshot(2)).unwrap();
        let before = book.clone();
        let mut orders = OrderState::new();
        let id = submit(&mut orders, OrderKind::Market, TimeInForce::Fok, Side::Buy, 20, 1);
        let outcome = execute_taker(&mut orders, &mut book, id, SweepConfig::default()).unwrap();
        assert_eq!(outcome.filled, QtyAtoms::new(0));
        assert_eq!(outcome.status, OrderStatus::Cancelled);
        assert_eq!(book, before);
    }

    #[test]
    fn level_and_quantity_caps_are_hard_sweep_limits() {
        let mut book = L2Book::new();
        book.apply_snapshot(snapshot(2)).unwrap();
        let mut orders = OrderState::new();
        let id = submit(&mut orders, OrderKind::Market, TimeInForce::Ioc, Side::Buy, 10, 1);
        let outcome = execute_taker(
            &mut orders,
            &mut book,
            id,
            SweepConfig {
                max_levels: 2,
                max_quantity: Some(QtyAtoms::new(5)),
            },
        )
        .unwrap();
        assert_eq!(outcome.filled, QtyAtoms::new(5));
        assert_eq!(outcome.levels, vec![level_fill(101, 3), level_fill(102, 2)]);
        assert_eq!(outcome.status, OrderStatus::Cancelled);
    }

    #[test]
    fn cancellation_policies_update_queue_ahead_deterministically() {
        let displayed = QtyAtoms::new(20);
        let cancelled = QtyAtoms::new(4);

        let mut conservative = QueuePosition::join(QtyAtoms::new(10));
        assert_eq!(
            conservative
                .apply_cancel(cancelled, displayed, CancelAllocationPolicy::Conservative)
                .unwrap(),
            QtyAtoms::new(0)
        );
        assert_eq!(conservative.ahead(), QtyAtoms::new(10));

        let mut optimistic = QueuePosition::join(QtyAtoms::new(10));
        assert_eq!(
            optimistic
                .apply_cancel(cancelled, displayed, CancelAllocationPolicy::Optimistic)
                .unwrap(),
            QtyAtoms::new(4)
        );
        assert_eq!(optimistic.ahead(), QtyAtoms::new(6));

        let mut pro_rata = QueuePosition::join(QtyAtoms::new(10));
        assert_eq!(
            pro_rata
                .apply_cancel(cancelled, displayed, CancelAllocationPolicy::ProRata)
                .unwrap(),
            QtyAtoms::new(2)
        );
        assert_eq!(pro_rata.ahead(), QtyAtoms::new(8));
        assert_eq!(pro_rata.apply_trade(QtyAtoms::new(9)), QtyAtoms::new(1));
    }

    #[test]
    fn resting_queue_fill_requires_later_trade_and_updates_order_atomically() {
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
        let mut queue = QueuePosition::join(QtyAtoms::new(3));
        assert_eq!(
            execute_resting_trade(
                &mut orders,
                &mut queue,
                id,
                10,
                PriceAtoms::new(100),
                QtyAtoms::new(5),
            )
            .unwrap(),
            QtyAtoms::new(0)
        );
        assert_eq!(queue.ahead(), QtyAtoms::new(3));
        assert_eq!(
            execute_resting_trade(
                &mut orders,
                &mut queue,
                id,
                11,
                PriceAtoms::new(100),
                QtyAtoms::new(5),
            )
            .unwrap(),
            QtyAtoms::new(2)
        );
        assert_eq!(queue.ahead(), QtyAtoms::new(0));
        assert_eq!(orders.get(id).unwrap().filled, QtyAtoms::new(2));
    }

    fn level_fill(price: i64, quantity: u64) -> LevelFill {
        LevelFill {
            price: PriceAtoms::new(price),
            quantity: QtyAtoms::new(quantity),
        }
    }
}
