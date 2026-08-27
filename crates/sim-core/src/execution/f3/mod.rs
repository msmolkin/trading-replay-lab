//! F3 market-by-order reconstruction and counterfactual player queue modeling.

use std::collections::{BTreeMap, BTreeSet};

use crate::numeric::{PriceAtoms, QtyAtoms};
use crate::orders::{OrderError, OrderId, OrderKind, OrderState, OrderStatus, Side, TimeInForce};

const MAX_SOURCE_ORDER_ID_BYTES: usize = 4096;
const PPM_DENOMINATOR: u128 = 1_000_000;

/// One visible source order from an authoritative MBO/L3 feed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VisibleOrder {
    /// Provider/venue order identity.
    pub source_order_id: String,
    /// Resting side (`Buy` is bid, `Sell` is ask).
    pub side: Side,
    /// Resting price.
    pub price: PriceAtoms,
    /// Remaining displayed quantity.
    pub quantity: QtyAtoms,
}

/// Full authoritative MBO snapshot in source priority order.
///
/// For orders sharing a side and price, vector order is queue priority from front to back.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MboSnapshot {
    /// Source sequence represented by the snapshot.
    pub sequence: u64,
    /// Complete visible source-order set.
    pub orders: Vec<VisibleOrder>,
}

/// One normalized source MBO action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MboAction {
    /// Add a new visible source order at the back of its price queue.
    Add(VisibleOrder),
    /// Replace visible price/quantity for an existing source order.
    Modify(VisibleOrder),
    /// Cancel the remaining visible source order.
    Cancel {
        /// Source order identity.
        source_order_id: String,
        /// Side declared by the source event.
        side: Side,
    },
    /// Consume quantity from one visible source order.
    Fill {
        /// Source order identity.
        source_order_id: String,
        /// Side declared by the source event.
        side: Side,
        /// Quantity consumed by this source fill.
        quantity: QtyAtoms,
    },
    /// Clear every visible source order on one side.
    Clear {
        /// Side cleared by the source event.
        side: Side,
    },
}

/// One sequence-addressed MBO event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MboEvent {
    /// Exact source sequence. It must be previous sequence plus one.
    pub sequence: u64,
    /// Source lifecycle action.
    pub action: MboAction,
}

/// Explicit uncertainty retained by the highest-fidelity model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum F3Uncertainty {
    /// A same-price MODIFY is assumed to retain its source queue priority.
    SamePriceModifyPriorityAssumed,
    /// A full recovery snapshot rebuilt source priority after prior continuity existed.
    ReconnectPriorityRebuilt,
    /// The player order is counterfactual and therefore can affect historical queue outcomes.
    CounterfactualPlayerImpact,
    /// The accepted player order improves the historical same-side BBO within its configured cap.
    PlayerBboImprovement,
    /// Historical flow reached a source order behind the player; the model therefore fills the
    /// player first while retaining the observed historical source event.
    HistoricalFillWouldCrossPlayer,
    /// A source-side clear invalidated the hypothetical player queue position.
    ClearInvalidatedPlayer,
}

/// Result of applying one authoritative source update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MboApplyOutcome {
    /// New authoritative source sequence.
    pub sequence: u64,
    /// Explicit model uncertainty introduced by this transition.
    pub uncertainty: Vec<F3Uncertainty>,
}

/// Best visible MBO price level summary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MboLevel {
    /// Level price.
    pub price: PriceAtoms,
    /// Sum of remaining source-order quantity at the level.
    pub quantity: QtyAtoms,
    /// Number of visible source orders at the level.
    pub order_count: usize,
}

/// Hard caps for one counterfactual player insertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayerImpactCaps {
    /// Maximum player order quantity.
    pub max_order_quantity: QtyAtoms,
    /// Maximum player quantity as parts-per-million of currently visible same-side quantity.
    pub max_side_fraction_ppm: u32,
    /// Maximum same-side BBO improvement allowed for a resting counterfactual order.
    pub max_bbo_improvement_atoms: u64,
    /// Maximum number of source orders that may be modeled ahead at the insertion price.
    pub max_source_orders_ahead: usize,
}

impl Default for PlayerImpactCaps {
    fn default() -> Self {
        Self {
            max_order_quantity: QtyAtoms::new(u64::MAX),
            max_side_fraction_ppm: 1_000_000,
            max_bbo_improvement_atoms: 0,
            max_source_orders_ahead: 100_000,
        }
    }
}

/// Counterfactual resting player state layered on top of the historical MBO book.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CounterfactualPlayer {
    order_id: OrderId,
    side: Side,
    price: PriceAtoms,
    ahead_source_orders: BTreeSet<String>,
    valid: bool,
}

impl CounterfactualPlayer {
    /// Simulator order represented by this queue insertion.
    #[must_use]
    pub const fn order_id(&self) -> OrderId {
        self.order_id
    }

    /// Resting side.
    #[must_use]
    pub const fn side(&self) -> Side {
        self.side
    }

    /// Resting limit price.
    #[must_use]
    pub const fn price(&self) -> PriceAtoms {
        self.price
    }

    /// Whether source continuity still supports this hypothetical queue position.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.valid
    }

    /// Number of historical source orders still modeled ahead of the player.
    #[must_use]
    pub fn ahead_order_count(&self) -> usize {
        self.ahead_source_orders.len()
    }

    /// Exact displayed source quantity still modeled ahead of the player.
    ///
    /// # Errors
    /// Returns [`F3Error::PlayerInvalidated`] after continuity/clear invalidation, or
    /// [`F3Error::InvalidQueue`] if reconstructed source facts no longer match the queue marker.
    pub fn ahead_quantity(&self, book: &MboBook) -> Result<QtyAtoms, F3Error> {
        if !self.valid || !book.enabled {
            return Err(F3Error::PlayerInvalidated);
        }
        let total = self.ahead_source_orders.iter().try_fold(0_u64, |total, source_id| {
            let order = book.orders.get(source_id).ok_or(F3Error::InvalidQueue)?;
            if order.side != self.side || order.price != self.price {
                return Err(F3Error::InvalidQueue);
            }
            total
                .checked_add(order.quantity.get())
                .ok_or(F3Error::QuantityArithmetic)
        })?;
        Ok(QtyAtoms::new(total))
    }

    fn invalidate(&mut self) {
        self.valid = false;
        self.ahead_source_orders.clear();
    }
}

/// Result of inserting a player behind the currently visible queue at its limit price.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlayerInsertion {
    /// Counterfactual queue marker.
    pub player: CounterfactualPlayer,
    /// Displayed source quantity initially ahead.
    pub ahead_quantity: QtyAtoms,
    /// Explicit counterfactual/impact uncertainty.
    pub uncertainty: Vec<F3Uncertainty>,
}

/// Combined result of one source event and any induced player fill.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct F3EventOutcome {
    /// Applied source sequence.
    pub sequence: u64,
    /// Quantity filled on the simulator player order by this source event.
    pub player_filled: QtyAtoms,
    /// Current simulator player order status.
    pub player_status: OrderStatus,
    /// Explicit source/counterfactual uncertainty.
    pub uncertainty: Vec<F3Uncertainty>,
}

/// Stable F3 reconstruction/player failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum F3Error {
    /// Snapshot/event contains invalid source-order facts or would cross the reconstructed book.
    InvalidBook,
    /// Exact source sequence continuity was lost; a complete recovery snapshot is required.
    SequenceGap,
    /// MBO reconstruction is quarantined until a valid full snapshot is installed.
    BookDisabled,
    /// An ADD reused an already-visible source order id.
    DuplicateSourceOrder,
    /// MODIFY/CANCEL/FILL referenced an unknown source order.
    UnknownSourceOrder,
    /// Event side disagreed with the authoritative visible source order.
    SideMismatch,
    /// Source fill exceeds that source order's remaining displayed quantity.
    SourceOverfill,
    /// Checked visible quantity arithmetic failed.
    QuantityArithmetic,
    /// Counterfactual queue facts became inconsistent with source reconstruction.
    InvalidQueue,
    /// Simulator order is not an eligible resting GTC limit order.
    UnsupportedPlayerOrder,
    /// Player insertion is behind the currently reconstructed source frontier.
    PlayerFrontierMismatch,
    /// Player quantity exceeds the configured absolute size cap.
    PlayerSizeCapExceeded,
    /// Player size as a share of same-side displayed depth exceeds its configured cap.
    PlayerImpactCapExceeded,
    /// Player BBO improvement exceeds its configured impact cap.
    PlayerBboImpactExceeded,
    /// Too many source orders are ahead for the configured queue cap.
    PlayerQueueCapExceeded,
    /// Source continuity/clear has invalidated the counterfactual queue marker.
    PlayerInvalidated,
    /// Authoritative simulator order transition failed.
    Order(OrderError),
}

impl core::fmt::Display for F3Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let message = match self {
            Self::InvalidBook => "invalid F3 market-by-order book",
            Self::SequenceGap => "F3 source sequence gap requires recovery snapshot",
            Self::BookDisabled => "F3 book is disabled until recovery snapshot",
            Self::DuplicateSourceOrder => "duplicate F3 source order id",
            Self::UnknownSourceOrder => "unknown F3 source order id",
            Self::SideMismatch => "F3 source event side mismatch",
            Self::SourceOverfill => "F3 source fill exceeds visible quantity",
            Self::QuantityArithmetic => "F3 quantity arithmetic failed",
            Self::InvalidQueue => "invalid F3 player queue state",
            Self::UnsupportedPlayerOrder => "unsupported F3 counterfactual player order",
            Self::PlayerFrontierMismatch => "F3 player submission is after visible source frontier",
            Self::PlayerSizeCapExceeded => "F3 player size cap exceeded",
            Self::PlayerImpactCapExceeded => "F3 player displayed-share cap exceeded",
            Self::PlayerBboImpactExceeded => "F3 player BBO impact cap exceeded",
            Self::PlayerQueueCapExceeded => "F3 player queue-ahead cap exceeded",
            Self::PlayerInvalidated => "F3 player queue was invalidated by source continuity",
            Self::Order(error) => return write!(formatter, "F3 order transition failed: {error}"),
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for F3Error {}

impl From<OrderError> for F3Error {
    fn from(value: OrderError) -> Self {
        Self::Order(value)
    }
}

/// Sequence-aware authoritative MBO reconstruction.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MboBook {
    sequence: Option<u64>,
    enabled: bool,
    orders: BTreeMap<String, VisibleOrder>,
    bids: BTreeMap<i64, Vec<String>>,
    asks: BTreeMap<i64, Vec<String>>,
}

impl MboBook {
    /// Creates an empty disabled book. A full snapshot is required before use.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sequence: None,
            enabled: false,
            orders: BTreeMap::new(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        }
    }

    /// Whether exact source continuity currently permits execution/queue use.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Current authoritative source sequence.
    #[must_use]
    pub const fn sequence(&self) -> Option<u64> {
        self.sequence
    }

    /// Visible source order by provider id.
    #[must_use]
    pub fn visible_order(&self, source_order_id: &str) -> Option<&VisibleOrder> {
        if !self.enabled {
            return None;
        }
        self.orders.get(source_order_id)
    }

    /// Source ids at one exact price in front-to-back queue order.
    #[must_use]
    pub fn order_ids_at(&self, side: Side, price: PriceAtoms) -> Vec<&str> {
        if !self.enabled {
            return Vec::new();
        }
        self.queue(side, price.get())
            .map_or_else(Vec::new, |ids| ids.iter().map(String::as_str).collect())
    }

    /// Best visible bid level while reconstruction is enabled.
    ///
    /// # Errors
    /// Returns [`F3Error::QuantityArithmetic`] if a visible level cannot be summed exactly.
    pub fn best_bid(&self) -> Result<Option<MboLevel>, F3Error> {
        if !self.enabled {
            return Ok(None);
        }
        self.bids
            .iter()
            .next_back()
            .map(|(&price, ids)| self.level(PriceAtoms::new(price), ids))
            .transpose()
    }

    /// Best visible ask level while reconstruction is enabled.
    ///
    /// # Errors
    /// Returns [`F3Error::QuantityArithmetic`] if a visible level cannot be summed exactly.
    pub fn best_ask(&self) -> Result<Option<MboLevel>, F3Error> {
        if !self.enabled {
            return Ok(None);
        }
        self.asks
            .iter()
            .next()
            .map(|(&price, ids)| self.level(PriceAtoms::new(price), ids))
            .transpose()
    }

    /// Exact displayed quantity on one side.
    ///
    /// # Errors
    /// Returns [`F3Error::QuantityArithmetic`] on checked accumulation failure.
    pub fn total_quantity(&self, side: Side) -> Result<u128, F3Error> {
        if !self.enabled {
            return Ok(0);
        }
        self.orders
            .values()
            .filter(|order| order.side == side)
            .try_fold(0_u128, |total, order| {
                total
                    .checked_add(u128::from(order.quantity.get()))
                    .ok_or(F3Error::QuantityArithmetic)
            })
    }

    /// Atomically installs a complete source snapshot and re-enables reconstruction.
    ///
    /// # Errors
    /// Returns [`F3Error::InvalidBook`] without changing the previous state for duplicate/empty
    /// ids, non-positive price/quantity, or a crossed reconstructed book.
    pub fn apply_snapshot(&mut self, snapshot: MboSnapshot) -> Result<MboApplyOutcome, F3Error> {
        let recovering = self.sequence.is_some();
        let mut next = Self::new();
        next.sequence = Some(snapshot.sequence);
        next.enabled = true;
        for order in snapshot.orders {
            next.validate_visible_order(&order)?;
            if next.orders.contains_key(&order.source_order_id) {
                return Err(F3Error::InvalidBook);
            }
            next.push_queue(&order);
            next.orders.insert(order.source_order_id.clone(), order);
        }
        next.validate_uncrossed()?;
        *self = next;
        Ok(MboApplyOutcome {
            sequence: snapshot.sequence,
            uncertainty: if recovering {
                vec![F3Uncertainty::ReconnectPriorityRebuilt]
            } else {
                Vec::new()
            },
        })
    }

    /// Applies one exact next-sequence MBO lifecycle event atomically.
    ///
    /// Continuity or structural failure disables the book while retaining prior source facts only
    /// for diagnostics. No query used for execution exposes those facts until a full snapshot
    /// recovers continuity.
    ///
    /// # Errors
    /// Returns a stable [`F3Error`]. Sequence/structural source errors quarantine reconstruction.
    pub fn apply_event(&mut self, event: MboEvent) -> Result<MboApplyOutcome, F3Error> {
        if !self.enabled {
            return Err(F3Error::BookDisabled);
        }
        let Some(sequence) = self.sequence else {
            self.enabled = false;
            return Err(F3Error::SequenceGap);
        };
        let Some(expected) = sequence.checked_add(1) else {
            self.enabled = false;
            return Err(F3Error::SequenceGap);
        };
        if event.sequence != expected {
            self.enabled = false;
            return Err(F3Error::SequenceGap);
        }

        let mut next = self.clone();
        let uncertainty = match next.apply_action(event.action) {
            Ok(uncertainty) => uncertainty,
            Err(error) => {
                self.enabled = false;
                return Err(error);
            }
        };
        if let Err(error) = next.validate_uncrossed() {
            self.enabled = false;
            return Err(error);
        }
        next.sequence = Some(event.sequence);
        *self = next;
        Ok(MboApplyOutcome {
            sequence: event.sequence,
            uncertainty,
        })
    }

    fn apply_action(&mut self, action: MboAction) -> Result<Vec<F3Uncertainty>, F3Error> {
        match action {
            MboAction::Add(order) => {
                self.validate_visible_order(&order)?;
                if self.orders.contains_key(&order.source_order_id) {
                    return Err(F3Error::DuplicateSourceOrder);
                }
                self.push_queue(&order);
                self.orders.insert(order.source_order_id.clone(), order);
                Ok(Vec::new())
            }
            MboAction::Modify(order) => self.modify(order),
            MboAction::Cancel {
                source_order_id,
                side,
            } => {
                let existing = self
                    .orders
                    .get(&source_order_id)
                    .ok_or(F3Error::UnknownSourceOrder)?
                    .clone();
                if existing.side != side {
                    return Err(F3Error::SideMismatch);
                }
                self.remove_source_order(&existing)?;
                Ok(Vec::new())
            }
            MboAction::Fill {
                source_order_id,
                side,
                quantity,
            } => {
                if quantity.get() == 0 {
                    return Err(F3Error::InvalidBook);
                }
                let existing = self
                    .orders
                    .get(&source_order_id)
                    .ok_or(F3Error::UnknownSourceOrder)?
                    .clone();
                if existing.side != side {
                    return Err(F3Error::SideMismatch);
                }
                if quantity > existing.quantity {
                    return Err(F3Error::SourceOverfill);
                }
                if quantity == existing.quantity {
                    self.remove_source_order(&existing)?;
                } else {
                    let order = self
                        .orders
                        .get_mut(&source_order_id)
                        .ok_or(F3Error::UnknownSourceOrder)?;
                    order.quantity = QtyAtoms::new(existing.quantity.get() - quantity.get());
                }
                Ok(Vec::new())
            }
            MboAction::Clear { side } => {
                self.clear_side(side);
                Ok(Vec::new())
            }
        }
    }

    fn modify(&mut self, replacement: VisibleOrder) -> Result<Vec<F3Uncertainty>, F3Error> {
        self.validate_visible_order(&replacement)?;
        let existing = self
            .orders
            .get(&replacement.source_order_id)
            .ok_or(F3Error::UnknownSourceOrder)?
            .clone();
        if existing.side != replacement.side {
            return Err(F3Error::SideMismatch);
        }
        if existing.price == replacement.price {
            let changed = existing.quantity != replacement.quantity;
            self.orders
                .insert(replacement.source_order_id.clone(), replacement);
            return Ok(if changed {
                vec![F3Uncertainty::SamePriceModifyPriorityAssumed]
            } else {
                Vec::new()
            });
        }
        self.remove_from_queue(existing.side, existing.price.get(), &existing.source_order_id)?;
        self.push_queue(&replacement);
        self.orders
            .insert(replacement.source_order_id.clone(), replacement);
        Ok(Vec::new())
    }

    fn validate_visible_order(&self, order: &VisibleOrder) -> Result<(), F3Error> {
        if order.source_order_id.is_empty()
            || order.source_order_id.len() > MAX_SOURCE_ORDER_ID_BYTES
            || order.price.get() <= 0
            || order.quantity.get() == 0
        {
            return Err(F3Error::InvalidBook);
        }
        Ok(())
    }

    fn validate_uncrossed(&self) -> Result<(), F3Error> {
        if let (Some((&bid, _)), Some((&ask, _))) =
            (self.bids.iter().next_back(), self.asks.iter().next())
        {
            if bid >= ask {
                return Err(F3Error::InvalidBook);
            }
        }
        Ok(())
    }

    fn queue(&self, side: Side, price: i64) -> Option<&Vec<String>> {
        match side {
            Side::Buy => self.bids.get(&price),
            Side::Sell => self.asks.get(&price),
        }
    }

    fn queue_mut(&mut self, side: Side) -> &mut BTreeMap<i64, Vec<String>> {
        match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        }
    }

    fn push_queue(&mut self, order: &VisibleOrder) {
        self.queue_mut(order.side)
            .entry(order.price.get())
            .or_default()
            .push(order.source_order_id.clone());
    }

    fn remove_source_order(&mut self, order: &VisibleOrder) -> Result<(), F3Error> {
        self.remove_from_queue(order.side, order.price.get(), &order.source_order_id)?;
        self.orders
            .remove(&order.source_order_id)
            .ok_or(F3Error::UnknownSourceOrder)?;
        Ok(())
    }

    fn remove_from_queue(
        &mut self,
        side: Side,
        price: i64,
        source_order_id: &str,
    ) -> Result<(), F3Error> {
        let queues = self.queue_mut(side);
        let ids = queues.get_mut(&price).ok_or(F3Error::InvalidBook)?;
        let position = ids
            .iter()
            .position(|candidate| candidate == source_order_id)
            .ok_or(F3Error::InvalidBook)?;
        ids.remove(position);
        if ids.is_empty() {
            queues.remove(&price);
        }
        Ok(())
    }

    fn clear_side(&mut self, side: Side) {
        let ids: Vec<String> = self
            .orders
            .values()
            .filter(|order| order.side == side)
            .map(|order| order.source_order_id.clone())
            .collect();
        for source_id in ids {
            self.orders.remove(&source_id);
        }
        self.queue_mut(side).clear();
    }

    fn level(&self, price: PriceAtoms, ids: &[String]) -> Result<MboLevel, F3Error> {
        let quantity = ids.iter().try_fold(0_u64, |total, source_id| {
            let order = self.orders.get(source_id).ok_or(F3Error::InvalidBook)?;
            total
                .checked_add(order.quantity.get())
                .ok_or(F3Error::QuantityArithmetic)
        })?;
        Ok(MboLevel {
            price,
            quantity: QtyAtoms::new(quantity),
            order_count: ids.len(),
        })
    }
}

/// Inserts one already-accepted simulator GTC limit order into the current MBO queue without
/// modifying historical source orders.
///
/// The order joins behind every source order currently visible at its price. Hard caps bound
/// absolute size, same-side displayed share, BBO improvement, and queue marker cardinality.
///
/// # Errors
/// Returns [`F3Error`] without mutating either authoritative order or source book state.
pub fn insert_player(
    orders: &OrderState,
    book: &MboBook,
    order_id: OrderId,
    caps: PlayerImpactCaps,
) -> Result<PlayerInsertion, F3Error> {
    if !book.enabled {
        return Err(F3Error::BookDisabled);
    }
    if caps.max_order_quantity.get() == 0 || caps.max_side_fraction_ppm > 1_000_000 {
        return Err(F3Error::PlayerImpactCapExceeded);
    }
    let order = orders.get(order_id).ok_or(OrderError::UnknownOrder)?;
    if !order.is_executable() || order.time_in_force != TimeInForce::Gtc {
        return Err(F3Error::UnsupportedPlayerOrder);
    }
    let OrderKind::Limit { limit_price } = order.kind else {
        return Err(F3Error::UnsupportedPlayerOrder);
    };
    let sequence = book.sequence.ok_or(F3Error::BookDisabled)?;
    if order.submitted_at_event_seq > sequence {
        return Err(F3Error::PlayerFrontierMismatch);
    }
    let remaining = order.remaining();
    if remaining.get() == 0 || remaining > caps.max_order_quantity {
        return Err(F3Error::PlayerSizeCapExceeded);
    }

    let side_total = book.total_quantity(order.side)?;
    if side_total == 0 {
        return Err(F3Error::PlayerImpactCapExceeded);
    }
    let scaled_player = u128::from(remaining.get())
        .checked_mul(PPM_DENOMINATOR)
        .ok_or(F3Error::QuantityArithmetic)?;
    let permitted = side_total
        .checked_mul(u128::from(caps.max_side_fraction_ppm))
        .ok_or(F3Error::QuantityArithmetic)?;
    if scaled_player > permitted {
        return Err(F3Error::PlayerImpactCapExceeded);
    }

    let improvement = bbo_improvement(book, order.side, limit_price)?;
    if improvement > caps.max_bbo_improvement_atoms {
        return Err(F3Error::PlayerBboImpactExceeded);
    }
    if is_marketable(book, order.side, limit_price)? {
        return Err(F3Error::UnsupportedPlayerOrder);
    }

    let ahead_ids = book
        .queue(order.side, limit_price.get())
        .cloned()
        .unwrap_or_default();
    if ahead_ids.len() > caps.max_source_orders_ahead {
        return Err(F3Error::PlayerQueueCapExceeded);
    }
    let ahead_source_orders: BTreeSet<String> = ahead_ids.into_iter().collect();
    let player = CounterfactualPlayer {
        order_id,
        side: order.side,
        price: limit_price,
        ahead_source_orders,
        valid: true,
    };
    let ahead_quantity = player.ahead_quantity(book)?;
    let mut uncertainty = vec![F3Uncertainty::CounterfactualPlayerImpact];
    if improvement > 0 {
        uncertainty.push(F3Uncertainty::PlayerBboImprovement);
    }
    Ok(PlayerInsertion {
        player,
        ahead_quantity,
        uncertainty,
    })
}

/// Applies one source MBO event and atomically updates an inserted player's queue/fill state.
///
/// If a continuity/structural source failure quarantines the book, the disabled source state and
/// invalidated player marker are committed even though this function returns an error. Successful
/// source events and player fills otherwise commit together.
///
/// # Errors
/// Returns [`F3Error`] for source reconstruction, invalidated player state, or simulator order
/// transition failures.
pub fn apply_event_with_player(
    orders: &mut OrderState,
    book: &mut MboBook,
    player: &mut CounterfactualPlayer,
    event: MboEvent,
) -> Result<F3EventOutcome, F3Error> {
    if !player.valid {
        return Err(F3Error::PlayerInvalidated);
    }
    let before = source_order_for_action(book, &event.action).cloned();
    let action = event.action.clone();
    let mut next_book = book.clone();
    let apply = match next_book.apply_event(event) {
        Ok(outcome) => outcome,
        Err(error) => {
            *book = next_book;
            player.invalidate();
            return Err(error);
        }
    };
    let mut next_orders = orders.clone();
    let mut next_player = player.clone();
    let mut uncertainty = apply.uncertainty;
    let player_fill = update_player_for_action(
        &mut next_orders,
        &next_book,
        &mut next_player,
        &action,
        before.as_ref(),
        &mut uncertainty,
    )?;
    let player_status = next_orders
        .get(next_player.order_id)
        .ok_or(OrderError::UnknownOrder)?
        .status;
    *orders = next_orders;
    *book = next_book;
    *player = next_player;
    Ok(F3EventOutcome {
        sequence: apply.sequence,
        player_filled: player_fill,
        player_status,
        uncertainty,
    })
}

fn source_order_for_action<'a>(book: &'a MboBook, action: &MboAction) -> Option<&'a VisibleOrder> {
    let source_id = match action {
        MboAction::Add(_) | MboAction::Clear { .. } => return None,
        MboAction::Modify(order) => order.source_order_id.as_str(),
        MboAction::Cancel {
            source_order_id, ..
        }
        | MboAction::Fill {
            source_order_id, ..
        } => source_order_id,
    };
    book.orders.get(source_id)
}

fn update_player_for_action(
    orders: &mut OrderState,
    book: &MboBook,
    player: &mut CounterfactualPlayer,
    action: &MboAction,
    before: Option<&VisibleOrder>,
    uncertainty: &mut Vec<F3Uncertainty>,
) -> Result<QtyAtoms, F3Error> {
    if let MboAction::Clear { side } = action {
        if *side == player.side {
            player.invalidate();
            uncertainty.push(F3Uncertainty::ClearInvalidatedPlayer);
        }
        return Ok(QtyAtoms::new(0));
    }

    let Some(before) = before else {
        return Ok(QtyAtoms::new(0));
    };
    let was_ahead = player.ahead_source_orders.contains(&before.source_order_id);
    let after = book.orders.get(&before.source_order_id);
    if was_ahead {
        let still_ahead = after.is_some_and(|order| order.side == player.side && order.price == player.price);
        if !still_ahead {
            player.ahead_source_orders.remove(&before.source_order_id);
        }
        return Ok(QtyAtoms::new(0));
    }

    let MboAction::Fill { quantity, .. } = action else {
        return Ok(QtyAtoms::new(0));
    };
    if before.side != player.side || before.price != player.price {
        return Ok(QtyAtoms::new(0));
    }
    if player.ahead_quantity(book)?.get() != 0 {
        return Ok(QtyAtoms::new(0));
    }
    let player_order = orders
        .get(player.order_id)
        .ok_or(OrderError::UnknownOrder)?
        .clone();
    if !player_order.is_executable() {
        return Ok(QtyAtoms::new(0));
    }
    let fill = QtyAtoms::new(quantity.get().min(player_order.remaining().get()));
    if fill.get() == 0 {
        return Ok(fill);
    }
    orders.record_fill(player.order_id, fill)?;
    uncertainty.push(F3Uncertainty::HistoricalFillWouldCrossPlayer);
    Ok(fill)
}

fn bbo_improvement(book: &MboBook, side: Side, price: PriceAtoms) -> Result<u64, F3Error> {
    match side {
        Side::Buy => {
            let best = book.best_bid()?.ok_or(F3Error::PlayerImpactCapExceeded)?;
            if price <= best.price {
                return Ok(0);
            }
            u64::try_from(price.get() - best.price.get()).map_err(|_| F3Error::QuantityArithmetic)
        }
        Side::Sell => {
            let best = book.best_ask()?.ok_or(F3Error::PlayerImpactCapExceeded)?;
            if price >= best.price {
                return Ok(0);
            }
            u64::try_from(best.price.get() - price.get()).map_err(|_| F3Error::QuantityArithmetic)
        }
    }
}

fn is_marketable(book: &MboBook, side: Side, price: PriceAtoms) -> Result<bool, F3Error> {
    match side {
        Side::Buy => Ok(book.best_ask()?.is_some_and(|ask| price >= ask.price)),
        Side::Sell => Ok(book.best_bid()?.is_some_and(|bid| price <= bid.price)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::NewOrder;

    fn source(id: &str, side: Side, price: i64, quantity: u64) -> VisibleOrder {
        VisibleOrder {
            source_order_id: id.into(),
            side,
            price: PriceAtoms::new(price),
            quantity: QtyAtoms::new(quantity),
        }
    }

    fn snapshot(sequence: u64) -> MboSnapshot {
        MboSnapshot {
            sequence,
            orders: vec![
                source("b1", Side::Buy, 100, 4),
                source("b2", Side::Buy, 100, 3),
                source("b3", Side::Buy, 99, 5),
                source("a1", Side::Sell, 101, 2),
                source("a2", Side::Sell, 101, 4),
                source("a3", Side::Sell, 102, 6),
            ],
        }
    }

    fn submit_player(
        orders: &mut OrderState,
        side: Side,
        price: i64,
        quantity: u64,
        submitted_at_event_seq: u64,
    ) -> OrderId {
        orders
            .submit(
                NewOrder {
                    client_order_id: "player".into(),
                    instrument_id: "SYNTH".into(),
                    side,
                    quantity: QtyAtoms::new(quantity),
                    kind: OrderKind::Limit {
                        limit_price: PriceAtoms::new(price),
                    },
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

    fn permissive_caps() -> PlayerImpactCaps {
        PlayerImpactCaps {
            max_order_quantity: QtyAtoms::new(100),
            max_side_fraction_ppm: 1_000_000,
            max_bbo_improvement_atoms: 10,
            max_source_orders_ahead: 100,
        }
    }

    #[test]
    fn snapshot_reconstructs_per_order_priority_and_top_levels() {
        let mut book = MboBook::new();
        let outcome = book.apply_snapshot(snapshot(10)).unwrap();
        assert!(outcome.uncertainty.is_empty());
        assert!(book.is_enabled());
        assert_eq!(book.sequence(), Some(10));
        assert_eq!(book.order_ids_at(Side::Buy, PriceAtoms::new(100)), vec!["b1", "b2"]);
        assert_eq!(
            book.best_bid().unwrap(),
            Some(MboLevel {
                price: PriceAtoms::new(100),
                quantity: QtyAtoms::new(7),
                order_count: 2,
            })
        );
        assert_eq!(
            book.best_ask().unwrap(),
            Some(MboLevel {
                price: PriceAtoms::new(101),
                quantity: QtyAtoms::new(6),
                order_count: 2,
            })
        );
    }

    #[test]
    fn add_modify_fill_cancel_and_clear_reconstruct_deterministically() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        book.apply_event(MboEvent {
            sequence: 11,
            action: MboAction::Add(source("b4", Side::Buy, 100, 2)),
        })
        .unwrap();
        assert_eq!(
            book.order_ids_at(Side::Buy, PriceAtoms::new(100)),
            vec!["b1", "b2", "b4"]
        );

        let modified = book
            .apply_event(MboEvent {
                sequence: 12,
                action: MboAction::Modify(source("b1", Side::Buy, 100, 6)),
            })
            .unwrap();
        assert_eq!(
            modified.uncertainty,
            vec![F3Uncertainty::SamePriceModifyPriorityAssumed]
        );
        assert_eq!(book.order_ids_at(Side::Buy, PriceAtoms::new(100))[0], "b1");

        book.apply_event(MboEvent {
            sequence: 13,
            action: MboAction::Fill {
                source_order_id: "b1".into(),
                side: Side::Buy,
                quantity: QtyAtoms::new(2),
            },
        })
        .unwrap();
        assert_eq!(book.visible_order("b1").unwrap().quantity, QtyAtoms::new(4));

        book.apply_event(MboEvent {
            sequence: 14,
            action: MboAction::Modify(source("b1", Side::Buy, 98, 4)),
        })
        .unwrap();
        assert_eq!(
            book.order_ids_at(Side::Buy, PriceAtoms::new(100)),
            vec!["b2", "b4"]
        );
        assert_eq!(book.order_ids_at(Side::Buy, PriceAtoms::new(98)), vec!["b1"]);

        book.apply_event(MboEvent {
            sequence: 15,
            action: MboAction::Cancel {
                source_order_id: "b2".into(),
                side: Side::Buy,
            },
        })
        .unwrap();
        assert_eq!(book.order_ids_at(Side::Buy, PriceAtoms::new(100)), vec!["b4"]);

        book.apply_event(MboEvent {
            sequence: 16,
            action: MboAction::Clear { side: Side::Sell },
        })
        .unwrap();
        assert_eq!(book.best_ask().unwrap(), None);
        assert_eq!(book.total_quantity(Side::Sell).unwrap(), 0);
    }

    #[test]
    fn sequence_gap_quarantines_until_reconnect_snapshot() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let prior = book.orders.clone();
        assert_eq!(
            book.apply_event(MboEvent {
                sequence: 12,
                action: MboAction::Cancel {
                    source_order_id: "b1".into(),
                    side: Side::Buy,
                },
            }),
            Err(F3Error::SequenceGap)
        );
        assert!(!book.is_enabled());
        assert_eq!(book.orders, prior);
        assert_eq!(book.best_bid().unwrap(), None);
        assert_eq!(
            book.apply_event(MboEvent {
                sequence: 11,
                action: MboAction::Clear { side: Side::Buy },
            }),
            Err(F3Error::BookDisabled)
        );

        let recovered = book.apply_snapshot(snapshot(20)).unwrap();
        assert!(book.is_enabled());
        assert_eq!(book.sequence(), Some(20));
        assert_eq!(
            recovered.uncertainty,
            vec![F3Uncertainty::ReconnectPriorityRebuilt]
        );
    }

    #[test]
    fn structural_source_error_disables_without_partial_application() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(1)).unwrap();
        let prior = book.orders.clone();
        assert_eq!(
            book.apply_event(MboEvent {
                sequence: 2,
                action: MboAction::Add(source("b1", Side::Buy, 98, 9)),
            }),
            Err(F3Error::DuplicateSourceOrder)
        );
        assert!(!book.is_enabled());
        assert_eq!(book.orders, prior);
    }

    #[test]
    fn player_joins_behind_exact_visible_queue_with_explicit_caps() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Sell, 101, 2, 10);
        let insertion = insert_player(&orders, &book, id, permissive_caps()).unwrap();
        assert_eq!(insertion.ahead_quantity, QtyAtoms::new(6));
        assert_eq!(insertion.player.ahead_order_count(), 2);
        assert_eq!(
            insertion.uncertainty,
            vec![F3Uncertainty::CounterfactualPlayerImpact]
        );
    }

    #[test]
    fn player_size_share_bbo_and_queue_caps_fail_closed() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let size_id = submit_player(&mut orders, Side::Buy, 100, 8, 10);
        assert_eq!(
            insert_player(
                &orders,
                &book,
                size_id,
                PlayerImpactCaps {
                    max_order_quantity: QtyAtoms::new(7),
                    ..permissive_caps()
                },
            ),
            Err(F3Error::PlayerSizeCapExceeded)
        );
        assert_eq!(
            insert_player(
                &orders,
                &book,
                size_id,
                PlayerImpactCaps {
                    max_side_fraction_ppm: 100_000,
                    ..permissive_caps()
                },
            ),
            Err(F3Error::PlayerImpactCapExceeded)
        );

        let improve_id = submit_player(&mut orders, Side::Buy, 101, 1, 10);
        assert_eq!(
            insert_player(
                &orders,
                &book,
                improve_id,
                PlayerImpactCaps {
                    max_bbo_improvement_atoms: 0,
                    ..permissive_caps()
                },
            ),
            Err(F3Error::PlayerBboImpactExceeded)
        );

        let queue_id = submit_player(&mut orders, Side::Sell, 101, 1, 10);
        assert_eq!(
            insert_player(
                &orders,
                &book,
                queue_id,
                PlayerImpactCaps {
                    max_source_orders_ahead: 1,
                    ..permissive_caps()
                },
            ),
            Err(F3Error::PlayerQueueCapExceeded)
        );
    }

    #[test]
    fn permitted_bbo_improvement_is_reported_as_uncertain() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Buy, 101, 1, 10);
        let insertion = insert_player(&orders, &book, id, permissive_caps()).unwrap_err();
        assert_eq!(insertion, F3Error::UnsupportedPlayerOrder);

        let id = submit_player(&mut orders, Side::Buy, 100, 1, 10);
        let insertion = insert_player(&orders, &book, id, permissive_caps()).unwrap();
        assert_eq!(
            insertion.uncertainty,
            vec![F3Uncertainty::CounterfactualPlayerImpact]
        );
    }

    #[test]
    fn historical_fill_behind_player_fills_player_after_ahead_queue_clears() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Sell, 101, 5, 10);
        let insertion = insert_player(&orders, &book, id, permissive_caps()).unwrap();
        let mut player = insertion.player;

        apply_event_with_player(
            &mut orders,
            &mut book,
            &mut player,
            MboEvent {
                sequence: 11,
                action: MboAction::Add(source("a4", Side::Sell, 101, 3)),
            },
        )
        .unwrap();
        apply_event_with_player(
            &mut orders,
            &mut book,
            &mut player,
            MboEvent {
                sequence: 12,
                action: MboAction::Fill {
                    source_order_id: "a1".into(),
                    side: Side::Sell,
                    quantity: QtyAtoms::new(2),
                },
            },
        )
        .unwrap();
        apply_event_with_player(
            &mut orders,
            &mut book,
            &mut player,
            MboEvent {
                sequence: 13,
                action: MboAction::Cancel {
                    source_order_id: "a2".into(),
                    side: Side::Sell,
                },
            },
        )
        .unwrap();
        assert_eq!(player.ahead_quantity(&book).unwrap(), QtyAtoms::new(0));

        let outcome = apply_event_with_player(
            &mut orders,
            &mut book,
            &mut player,
            MboEvent {
                sequence: 14,
                action: MboAction::Fill {
                    source_order_id: "a4".into(),
                    side: Side::Sell,
                    quantity: QtyAtoms::new(3),
                },
            },
        )
        .unwrap();
        assert_eq!(outcome.player_filled, QtyAtoms::new(3));
        assert_eq!(outcome.player_status, OrderStatus::PartiallyFilled);
        assert!(
            outcome
                .uncertainty
                .contains(&F3Uncertainty::HistoricalFillWouldCrossPlayer)
        );
        assert_eq!(orders.get(id).unwrap().filled, QtyAtoms::new(3));
    }

    #[test]
    fn source_clear_invalidates_counterfactual_player() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Sell, 101, 1, 10);
        let mut player = insert_player(&orders, &book, id, permissive_caps())
            .unwrap()
            .player;
        let outcome = apply_event_with_player(
            &mut orders,
            &mut book,
            &mut player,
            MboEvent {
                sequence: 11,
                action: MboAction::Clear { side: Side::Sell },
            },
        )
        .unwrap();
        assert!(!player.is_valid());
        assert_eq!(outcome.player_filled, QtyAtoms::new(0));
        assert!(outcome.uncertainty.contains(&F3Uncertainty::ClearInvalidatedPlayer));
    }

    #[test]
    fn source_gap_invalidates_player_and_commits_quarantine() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Buy, 100, 1, 10);
        let mut player = insert_player(&orders, &book, id, permissive_caps())
            .unwrap()
            .player;
        assert_eq!(
            apply_event_with_player(
                &mut orders,
                &mut book,
                &mut player,
                MboEvent {
                    sequence: 12,
                    action: MboAction::Clear { side: Side::Sell },
                },
            ),
            Err(F3Error::SequenceGap)
        );
        assert!(!book.is_enabled());
        assert!(!player.is_valid());
        assert_eq!(orders.get(id).unwrap().filled, QtyAtoms::new(0));
    }
}
