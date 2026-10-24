use std::collections::BTreeSet;

use crate::numeric::{PriceAtoms, QtyAtoms};
use crate::orders::{OrderError, OrderId, OrderStatus, Side};

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MboSnapshot {
    /// Source sequence represented by the snapshot.
    pub sequence: u64,
    /// Complete visible order set. Equal-price vector order is front-to-back priority.
    pub orders: Vec<VisibleOrder>,
}

/// One normalized source MBO action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MboAction {
    /// Add a new visible source order at the back of its price queue.
    Add(VisibleOrder),
    /// Replace price/quantity for an existing source order.
    Modify(VisibleOrder),
    /// Cancel an existing source order.
    Cancel {
        /// Source order identity.
        source_order_id: String,
        /// Side declared by the source event.
        side: Side,
    },
    /// Consume quantity from one existing source order.
    Fill {
        /// Source order identity.
        source_order_id: String,
        /// Side declared by the source event.
        side: Side,
        /// Quantity consumed by this source fill.
        quantity: QtyAtoms,
    },
    /// Clear all visible source orders on one side.
    Clear {
        /// Side cleared by the source event.
        side: Side,
    },
}

/// One sequence-addressed MBO event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MboEvent {
    /// Exact source sequence; it must be the previous sequence plus one.
    pub sequence: u64,
    /// Source lifecycle action.
    pub action: MboAction,
}

/// Explicit uncertainty retained by the highest-fidelity model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum F3Uncertainty {
    /// A same-price MODIFY is assumed to retain source queue priority.
    SamePriceModifyPriorityAssumed,
    /// A recovery snapshot rebuilt source priority after prior continuity existed.
    ReconnectPriorityRebuilt,
    /// The player order is counterfactual and can affect historical queue outcomes.
    CounterfactualPlayerImpact,
    /// The player improves the historical same-side BBO within its configured cap.
    PlayerBboImprovement,
    /// Historical flow reached a source order behind the player.
    HistoricalFillWouldCrossPlayer,
    /// A source-side clear invalidated the hypothetical player queue position.
    ClearInvalidatedPlayer,
}

/// Result of applying one authoritative source update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MboApplyOutcome {
    /// New authoritative source sequence.
    pub sequence: u64,
    /// Explicit uncertainty introduced by this transition.
    pub uncertainty: Vec<F3Uncertainty>,
}

/// Best visible MBO price-level summary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MboLevel {
    /// Level price.
    pub price: PriceAtoms,
    /// Sum of source-order quantity at the level.
    pub quantity: QtyAtoms,
    /// Number of visible source orders at the level.
    pub order_count: usize,
}

/// Hard caps for one counterfactual player insertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlayerImpactCaps {
    /// Maximum player order quantity.
    pub max_order_quantity: QtyAtoms,
    /// Maximum player quantity as parts-per-million of visible same-side quantity.
    pub max_side_fraction_ppm: u32,
    /// Maximum same-side BBO improvement for a resting player order.
    pub max_bbo_improvement_atoms: u64,
    /// Maximum number of source orders modeled ahead at the insertion price.
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

/// Counterfactual resting player state layered on the historical MBO book.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CounterfactualPlayer {
    pub(super) order_id: OrderId,
    pub(super) side: Side,
    pub(super) price: PriceAtoms,
    pub(super) ahead_source_orders: BTreeSet<String>,
    pub(super) valid: bool,
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

    /// Whether source continuity still supports this queue position.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.valid
    }

    /// Number of historical source orders still modeled ahead.
    #[must_use]
    pub fn ahead_order_count(&self) -> usize {
        self.ahead_source_orders.len()
    }

    pub(super) fn invalidate(&mut self) {
        self.valid = false;
        self.ahead_source_orders.clear();
    }
}

/// Result of inserting a player behind the currently visible queue.
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
    /// Snapshot/event contains invalid source-order facts or a crossed book.
    InvalidBook,
    /// Exact source sequence continuity was lost.
    SequenceGap,
    /// Reconstruction is quarantined until a full snapshot is installed.
    BookDisabled,
    /// An ADD reused an already-visible source order id.
    DuplicateSourceOrder,
    /// MODIFY/CANCEL/FILL referenced an unknown source order.
    UnknownSourceOrder,
    /// Event side disagreed with the visible source order.
    SideMismatch,
    /// Source fill exceeds remaining displayed quantity.
    SourceOverfill,
    /// Checked visible quantity arithmetic failed.
    QuantityArithmetic,
    /// Counterfactual queue facts became inconsistent.
    InvalidQueue,
    /// Simulator order is not an eligible resting GTC limit order.
    UnsupportedPlayerOrder,
    /// Player submission is after the visible source frontier.
    PlayerFrontierMismatch,
    /// Player quantity exceeds its absolute cap.
    PlayerSizeCapExceeded,
    /// Player displayed share exceeds its configured cap.
    PlayerImpactCapExceeded,
    /// Player BBO improvement exceeds its configured cap.
    PlayerBboImpactExceeded,
    /// Too many source orders are ahead for the configured cap.
    PlayerQueueCapExceeded,
    /// Source continuity/clear invalidated the player marker.
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
