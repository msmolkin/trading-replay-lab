//! Public versioned facade types and stable error classifications.

use crate::economics::{EconomicsError, LiquidityRole, ScheduledEconomicId};
use crate::execution::f0::{Bar, F0Config, F0Error};
use crate::execution::f1::{BboQuote, F1Config, F1Error, TradePrint};
use crate::execution::f2::{F2Error, L2Delta, L2Snapshot, SweepConfig};
use crate::kernel::{InputEnvelope, KernelError, KernelEvent};
use crate::ledger::{LedgerError, LedgerSnapshot};
use crate::numeric::{MoneyMinor, PriceAtoms, QtyAtoms, RatePpb};
use crate::orders::{NewOrder, Order, OrderError, OrderId, OrderStatus, ReplaceOrder, TopOfBook};
use crate::positions::{Position, PositionError, PositionMath};
use crate::risk::{Leverage, LiquidationState, MarginSnapshot, RiskError, RiskProfile};

/// Current public simulator facade API version.
pub const FACADE_API_VERSION: u16 = 1;

/// Fidelity tier selected and locked at session setup.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionTier {
    /// Completed OHLCV bars.
    F0,
    /// Trade-only tier. Reserved until a dedicated facade model is implemented.
    F0T,
    /// Trades plus BBO.
    F1,
    /// L2 snapshots/deltas.
    F2,
    /// MBO/L3. Implemented by M1-08, not a dependency of this facade version.
    F3,
}

impl ExecutionTier {
    /// Stable wire/debug code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::F0 => "F0",
            Self::F0T => "F0T",
            Self::F1 => "F1",
            Self::F2 => "F2",
            Self::F3 => "F3",
        }
    }
}

/// Compile-time execution-model availability for this facade version.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExecutionModelRegistry;

impl ExecutionModelRegistry {
    /// Returns whether M1-11 can dispatch this tier without fabricating fidelity.
    #[must_use]
    pub const fn supports(self, tier: ExecutionTier) -> bool {
        matches!(
            tier,
            ExecutionTier::F0 | ExecutionTier::F1 | ExecutionTier::F2
        )
    }

    /// Returns supported tiers in stable fidelity order.
    #[must_use]
    pub const fn supported(self) -> [ExecutionTier; 3] {
        [ExecutionTier::F0, ExecutionTier::F1, ExecutionTier::F2]
    }
}

/// Immutable rules bound into every simulator snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FacadeRules {
    /// Exact position P&L/basis math.
    pub position_math: PositionMath,
    /// Exact margin/maintenance/fee notional math.
    pub risk_profile: RiskProfile,
    /// F0 fill fee classification when bar data cannot establish maker/taker.
    pub f0_fee_role: LiquidityRole,
    /// Maker fee/rebate rate in signed PPB.
    pub maker_fee_rate: RatePpb,
    /// Taker fee/rebate rate in signed PPB.
    pub taker_fee_rate: RatePpb,
}

/// Immutable session binding and execution rules.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FacadeConfig {
    /// Public compatibility version.
    pub api_version: u16,
    /// Session identity required on every [`InputEnvelope`].
    pub session_id: String,
    /// Single authoritative instrument handled by this facade instance.
    pub instrument_id: String,
    /// Locked execution tier.
    pub execution_tier: ExecutionTier,
    /// Exact economics/risk rules.
    pub rules: FacadeRules,
}

impl FacadeConfig {
    pub(crate) fn validate(&self) -> Result<(), FacadeError> {
        if self.api_version != FACADE_API_VERSION {
            return Err(FacadeError::new(FacadeErrorCode::UnsupportedApiVersion));
        }
        if self.session_id.is_empty() || self.instrument_id.is_empty() {
            return Err(FacadeError::new(FacadeErrorCode::InvalidConfiguration));
        }
        if !ExecutionModelRegistry.supports(self.execution_tier) {
            return Err(FacadeError::new(FacadeErrorCode::UnsupportedExecutionTier));
        }
        self.rules
            .risk_profile
            .validate()
            .map_err(FacadeError::from_risk)?;
        if !math_is_consistent(self.rules.position_math, self.rules.risk_profile) {
            return Err(FacadeError::new(FacadeErrorCode::InvalidConfiguration));
        }
        Ok(())
    }
}

/// Validated starting state before the first accepted input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FacadeInitialState {
    /// Current position, usually flat.
    pub position: Position,
    /// Balanced current ledger state.
    pub ledger: LedgerSnapshot,
    /// Initial isolated leverage.
    pub leverage: Leverage,
}

/// Stable machine-readable facade error codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FacadeErrorCode {
    /// Facade/snapshot API version is unsupported.
    UnsupportedApiVersion,
    /// Snapshot format version is unsupported.
    UnsupportedSnapshotVersion,
    /// Config is empty or internally inconsistent.
    InvalidConfiguration,
    /// Input session differs from the bound session.
    SessionMismatch,
    /// Logical replay time moved backward.
    LogicalTimeRegression,
    /// Canonical input kind is unknown to this API version.
    UnsupportedInputKind,
    /// Canonical input payload is malformed, non-canonical, or has trailing bytes.
    InvalidPayload,
    /// Configured execution fidelity is not implemented by this facade version.
    UnsupportedExecutionTier,
    /// Input belongs to a different execution tier than the session.
    ExecutionTierMismatch,
    /// Order/instrument combination is incompatible with the requested operation.
    UnsupportedOrderCombination,
    /// Order references a different instrument than the locked session instrument.
    InstrumentMismatch,
    /// Market-data sequence regressed relative to the visible frontier.
    MarketSequenceRegression,
    /// Referenced order does not exist.
    UnknownOrder,
    /// Kernel sequence/state-version/hash transition failed.
    KernelTransition,
    /// Authoritative order lifecycle rejected a transition.
    OrderTransition,
    /// Position accounting failed.
    PositionAccounting,
    /// Ledger restore/posting failed.
    LedgerTransition,
    /// Scheduled economics/fee processing failed.
    EconomicsTransition,
    /// Margin/leverage/liquidation transition failed.
    RiskTransition,
    /// F0 execution failed.
    F0Execution,
    /// F1 execution failed.
    F1Execution,
    /// F2 reconstruction/execution failed.
    F2Execution,
    /// Snapshot contents violate authoritative invariants.
    InvalidSnapshot,
    /// Event ordinal/canonical length conversion overflowed.
    CounterOverflow,
}

/// Facade failure carrying only stable public classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FacadeError {
    /// Stable machine-readable failure code.
    pub code: FacadeErrorCode,
}

impl FacadeError {
    pub(crate) const fn new(code: FacadeErrorCode) -> Self {
        Self { code }
    }

    pub(crate) fn from_kernel(_error: KernelError) -> Self {
        Self::new(FacadeErrorCode::KernelTransition)
    }

    pub(crate) fn from_order(error: OrderError) -> Self {
        if error == OrderError::UnknownOrder {
            return Self::new(FacadeErrorCode::UnknownOrder);
        }
        if matches!(
            error,
            OrderError::ImmediateOrderRequiresAtomicExecution
                | OrderError::NotImmediateOrder
                | OrderError::PartialFokFill
        ) {
            return Self::new(FacadeErrorCode::UnsupportedOrderCombination);
        }
        Self::new(FacadeErrorCode::OrderTransition)
    }

    pub(crate) fn from_position(_error: PositionError) -> Self {
        Self::new(FacadeErrorCode::PositionAccounting)
    }

    pub(crate) fn from_ledger(_error: LedgerError) -> Self {
        Self::new(FacadeErrorCode::LedgerTransition)
    }

    pub(crate) fn from_economics(_error: EconomicsError) -> Self {
        Self::new(FacadeErrorCode::EconomicsTransition)
    }

    pub(crate) fn from_risk(_error: RiskError) -> Self {
        Self::new(FacadeErrorCode::RiskTransition)
    }

    pub(crate) fn from_f0(_error: F0Error) -> Self {
        Self::new(FacadeErrorCode::F0Execution)
    }

    pub(crate) fn from_f1(error: F1Error) -> Self {
        if matches!(
            error,
            F1Error::Order(
                OrderError::ImmediateOrderRequiresAtomicExecution
                    | OrderError::NotImmediateOrder
                    | OrderError::PartialFokFill
            )
        ) {
            return Self::new(FacadeErrorCode::UnsupportedOrderCombination);
        }
        Self::new(FacadeErrorCode::F1Execution)
    }

    pub(crate) fn from_f2(error: F2Error) -> Self {
        if matches!(
            error,
            F2Error::Order(
                OrderError::ImmediateOrderRequiresAtomicExecution
                    | OrderError::NotImmediateOrder
                    | OrderError::PartialFokFill
            )
        ) {
            return Self::new(FacadeErrorCode::UnsupportedOrderCombination);
        }
        Self::new(FacadeErrorCode::F2Execution)
    }
}

impl core::fmt::Display for FacadeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "simulator facade error: {:?}", self.code)
    }
}

impl std::error::Error for FacadeError {}

/// Submission input including the quote required for post/marketable-only validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmitOrderInput {
    /// Canonical order request.
    pub request: NewOrder,
    /// Visible quote at command acceptance when a liquidity constraint requires one.
    pub quote: Option<TopOfBook>,
}

/// Replacement input including the current visible quote when needed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplaceOrderInput {
    /// Numeric order id returned by an earlier submission event.
    pub order_id: u64,
    /// Replacement total quantity/kind.
    pub replacement: ReplaceOrder,
    /// Current visible quote for liquidity constraints.
    pub quote: Option<TopOfBook>,
}

/// Funding input whose event sequence is always assigned by the authoritative kernel cause.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FundingInput {
    /// Stable provider/source event identity.
    pub id: ScheduledEconomicId,
    /// Signed cash delta; positive is received and negative is paid.
    pub cash_delta: MoneyMinor,
}

/// Canonical v1 input payload types. Helpers encode these into opaque kernel payload bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FacadeInput {
    /// Submit one canonical order.
    SubmitOrder(SubmitOrderInput),
    /// Cancel an order by simulator id.
    CancelOrder { order_id: u64 },
    /// Replace total quantity/kind while preserving identity/fills.
    ReplaceOrder(ReplaceOrderInput),
    /// Offer one completed bar to one F0 order.
    ExecuteF0 {
        order_id: u64,
        bar: Bar,
        config: F0Config,
    },
    /// Offer one BBO quote to one F1 order.
    ExecuteF1Quote {
        order_id: u64,
        quote: BboQuote,
        config: F1Config,
    },
    /// Offer one later trade print to one resting F1 limit order.
    ExecuteF1Trade {
        order_id: u64,
        trade: TradePrint,
        eligible_after_event_seq: u64,
        displayed_ahead: QtyAtoms,
        config: F1Config,
    },
    /// Establish/recover the F2 book from a complete snapshot.
    F2Snapshot(L2Snapshot),
    /// Apply one sequence-contiguous absolute L2 delta.
    F2Delta(L2Delta),
    /// Execute one order against current reconstructed F2 depth.
    ExecuteF2 { order_id: u64, config: SweepConfig },
    /// Change isolated leverage atomically using an authoritative equity/mark view.
    SetLeverage {
        leverage: u8,
        equity: MoneyMinor,
        mark_price: PriceAtoms,
    },
    /// Evaluate deterministic maintenance liquidation using an authoritative equity/mark view.
    EvaluateRisk {
        equity: MoneyMinor,
        mark_price: PriceAtoms,
    },
    /// Post one idempotent signed funding cash flow.
    Funding(FundingInput),
}

/// One normalized execution component used by downstream read models.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionFill {
    /// Fill quantity.
    pub quantity: QtyAtoms,
    /// Fill price.
    pub price: PriceAtoms,
    /// Fee/liquidity classification used for this fill.
    pub liquidity_role: LiquidityRole,
}

/// Typed deterministic domain output. Every event is caused by exactly one kernel event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DomainEventPayload {
    /// Accepted order snapshot.
    OrderSubmitted(Order),
    /// Terminal cancellation.
    OrderCancelled { order_id: OrderId },
    /// Replaced order snapshot.
    OrderReplaced(Order),
    /// One execution attempt, including zero-fill attempts.
    Execution {
        order_id: OrderId,
        fills: Vec<ExecutionFill>,
        status: OrderStatus,
        uncertainty: Vec<String>,
    },
    /// Economic position after one or more fills from the same input.
    PositionChanged(Position),
    /// Exact fee/rebate posted for one fill.
    FeePosted {
        role: LiquidityRole,
        amount: MoneyMinor,
    },
    /// F2 complete snapshot accepted.
    DepthSnapshotApplied { sequence: u64 },
    /// F2 delta accepted.
    DepthDeltaApplied { sequence: u64 },
    /// Isolated leverage changed after a successful precheck.
    LeverageChanged {
        leverage: u8,
        margin: MarginSnapshot,
    },
    /// Maintenance state evaluated.
    RiskEvaluated {
        margin: MarginSnapshot,
        liquidation_state: LiquidationState,
        triggered: bool,
    },
    /// Funding was posted or recognized as an exact idempotent retry.
    FundingProcessed {
        posted: bool,
        cash_delta: MoneyMinor,
    },
}

/// One public domain event with deterministic ordinal under its causal input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainEvent {
    /// Kernel chain event proving input order/version/hash.
    pub cause: KernelEvent,
    /// Zero-based domain event ordinal under this input.
    pub ordinal: u32,
    /// Typed domain result.
    pub payload: DomainEventPayload,
}

fn math_is_consistent(position: PositionMath, risk: RiskProfile) -> bool {
    let economics = risk.math;
    position.contract_multiplier_atoms == economics.contract_multiplier_atoms
        && position.qty_scale == economics.qty_scale
        && position.price_scale == economics.price_scale
        && position.multiplier_scale == economics.multiplier_scale
        && position.settlement_scale == economics.settlement_scale
        && position.rounding == economics.rounding
}
