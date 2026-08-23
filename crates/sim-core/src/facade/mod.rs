//! Versioned public simulator facade over authoritative domain state.

use crate::economics::{
    EconomicsError, EconomicsState, ExecutionFeeInput, LiquidityRole, ScheduledCashFlow,
    ScheduledEconomicId,
};
use crate::execution::f0::{
    Bar, F0Config, F0Error, IntrabarPolicy, UncertaintyFlag, execute_bar,
};
use crate::execution::f1::{
    BboQuote, DisplayedAheadQueue, F1Config, F1Error, F1LiquidityRole, F1Uncertainty, TradePrint,
    execute_on_quote, execute_resting_on_trade,
};
use crate::execution::f2::{
    BookSide, DepthLevel, F2Error, L2Book, L2Delta, L2Snapshot, SweepConfig, execute_taker,
};
use crate::hash::CanonicalWriter;
use crate::kernel::{InputEnvelope, Kernel, KernelError, KernelEvent};
use crate::ledger::{Ledger, LedgerError, LedgerSnapshot};
use crate::numeric::{MoneyMinor, PriceAtoms, QtyAtoms, RatePpb};
use crate::orders::{
    NewOrder, Order, OrderError, OrderId, OrderKind, OrderState, OrderStatus, ReplaceOrder, Side,
    TimeInForce, TopOfBook,
};
use crate::positions::{FillSide, Position, PositionError, PositionMath};
use crate::risk::{
    Leverage, LiquidationState, MarginSnapshot, RiskError, RiskProfile, RiskState, margin_snapshot,
};
use crate::snapshot::{SNAPSHOT_FORMAT_VERSION, SimulatorSnapshot};

/// Current public simulator facade API version.
pub const FACADE_API_VERSION: u16 = 1;
const PAYLOAD_TAG: &[u8] = b"TRL-FACADE-PAYLOAD-v1\0";
const MAX_TEXT_BYTES: usize = 4096;
const MAX_DEPTH_LEVELS_PER_INPUT: usize = 100_000;

const KIND_ORDER_SUBMIT: &str = "ORDER_SUBMIT_V1";
const KIND_ORDER_CANCEL: &str = "ORDER_CANCEL_V1";
const KIND_ORDER_REPLACE: &str = "ORDER_REPLACE_V1";
const KIND_EXECUTE_F0: &str = "EXECUTE_F0_BAR_V1";
const KIND_EXECUTE_F1_QUOTE: &str = "EXECUTE_F1_QUOTE_V1";
const KIND_EXECUTE_F1_TRADE: &str = "EXECUTE_F1_TRADE_V1";
const KIND_F2_SNAPSHOT: &str = "F2_SNAPSHOT_V1";
const KIND_F2_DELTA: &str = "F2_DELTA_V1";
const KIND_EXECUTE_F2: &str = "EXECUTE_F2_V1";
const KIND_SET_LEVERAGE: &str = "RISK_SET_LEVERAGE_V1";
const KIND_EVALUATE_RISK: &str = "RISK_EVALUATE_V1";
const KIND_FUNDING: &str = "ECON_FUNDING_V1";

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
        matches!(tier, ExecutionTier::F0 | ExecutionTier::F1 | ExecutionTier::F2)
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
    fn validate(&self) -> Result<(), FacadeError> {
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
    const fn new(code: FacadeErrorCode) -> Self {
        Self { code }
    }

    fn from_kernel(_error: KernelError) -> Self {
        Self::new(FacadeErrorCode::KernelTransition)
    }

    fn from_order(error: OrderError) -> Self {
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

    fn from_position(_error: PositionError) -> Self {
        Self::new(FacadeErrorCode::PositionAccounting)
    }

    fn from_ledger(_error: LedgerError) -> Self {
        Self::new(FacadeErrorCode::LedgerTransition)
    }

    fn from_economics(_error: EconomicsError) -> Self {
        Self::new(FacadeErrorCode::EconomicsTransition)
    }

    fn from_risk(_error: RiskError) -> Self {
        Self::new(FacadeErrorCode::RiskTransition)
    }

    fn from_f0(_error: F0Error) -> Self {
        Self::new(FacadeErrorCode::F0Execution)
    }

    fn from_f1(error: F1Error) -> Self {
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

    fn from_f2(error: F2Error) -> Self {
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
    ExecuteF2 {
        order_id: u64,
        config: SweepConfig,
    },
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
    Funding(ScheduledCashFlow),
}

impl FacadeInput {
    /// Stable kernel input kind for this payload.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::SubmitOrder(_) => KIND_ORDER_SUBMIT,
            Self::CancelOrder { .. } => KIND_ORDER_CANCEL,
            Self::ReplaceOrder(_) => KIND_ORDER_REPLACE,
            Self::ExecuteF0 { .. } => KIND_EXECUTE_F0,
            Self::ExecuteF1Quote { .. } => KIND_EXECUTE_F1_QUOTE,
            Self::ExecuteF1Trade { .. } => KIND_EXECUTE_F1_TRADE,
            Self::F2Snapshot(_) => KIND_F2_SNAPSHOT,
            Self::F2Delta(_) => KIND_F2_DELTA,
            Self::ExecuteF2 { .. } => KIND_EXECUTE_F2,
            Self::SetLeverage { .. } => KIND_SET_LEVERAGE,
            Self::EvaluateRisk { .. } => KIND_EVALUATE_RISK,
            Self::Funding(_) => KIND_FUNDING,
        }
    }

    /// Canonical domain-separated binary payload consumed by [`SimulatorFacade::apply`].
    #[must_use]
    pub fn canonical_payload(&self) -> Vec<u8> {
        let mut writer = CanonicalWriter::new();
        writer.tag(PAYLOAD_TAG);
        match self {
            Self::SubmitOrder(input) => {
                write_new_order(&mut writer, &input.request);
                write_quote(&mut writer, input.quote);
            }
            Self::CancelOrder { order_id } => writer.u64(*order_id),
            Self::ReplaceOrder(input) => {
                writer.u64(input.order_id);
                writer.u64(input.replacement.quantity.get());
                write_order_kind(&mut writer, input.replacement.kind);
                write_quote(&mut writer, input.quote);
            }
            Self::ExecuteF0 {
                order_id,
                bar,
                config,
            } => {
                writer.u64(*order_id);
                writer.u64(bar.event_seq);
                writer.i64(bar.open.get());
                writer.i64(bar.high.get());
                writer.i64(bar.low.get());
                writer.i64(bar.close.get());
                writer.u64(bar.base_volume.get());
                match config.intrabar_policy {
                    IntrabarPolicy::Pessimistic => writer.text("PESSIMISTIC"),
                    IntrabarPolicy::Optimistic => writer.text("OPTIMISTIC"),
                    IntrabarPolicy::Seeded { seed } => {
                        writer.text("SEEDED");
                        writer.u64(seed);
                    }
                }
                writer.u64(config.market_slippage_atoms);
            }
            Self::ExecuteF1Quote {
                order_id,
                quote,
                config,
            } => {
                writer.u64(*order_id);
                write_bbo(&mut writer, *quote);
                write_f1_config(&mut writer, *config);
            }
            Self::ExecuteF1Trade {
                order_id,
                trade,
                eligible_after_event_seq,
                displayed_ahead,
                config,
            } => {
                writer.u64(*order_id);
                write_trade(&mut writer, *trade);
                writer.u64(*eligible_after_event_seq);
                writer.u64(displayed_ahead.get());
                write_f1_config(&mut writer, *config);
            }
            Self::F2Snapshot(snapshot) => {
                writer.u64(snapshot.sequence);
                write_levels(&mut writer, &snapshot.bids);
                write_levels(&mut writer, &snapshot.asks);
            }
            Self::F2Delta(delta) => {
                writer.u64(delta.previous_sequence);
                writer.u64(delta.sequence);
                writer.text(match delta.side {
                    BookSide::Bid => "BID",
                    BookSide::Ask => "ASK",
                });
                writer.i64(delta.price.get());
                writer.u64(delta.quantity.get());
            }
            Self::ExecuteF2 { order_id, config } => {
                writer.u64(*order_id);
                writer.u64(u64::try_from(config.max_levels).unwrap_or(u64::MAX));
                write_optional_qty(&mut writer, config.max_quantity);
            }
            Self::SetLeverage {
                leverage,
                equity,
                mark_price,
            } => {
                writer.u64(u64::from(*leverage));
                writer.i64(equity.get());
                writer.i64(mark_price.get());
            }
            Self::EvaluateRisk { equity, mark_price } => {
                writer.i64(equity.get());
                writer.i64(mark_price.get());
            }
            Self::Funding(flow) => {
                writer.text(&flow.id.source);
                writer.text(&flow.id.event_id);
                writer.i64(flow.cash_delta.get());
            }
        }
        writer.finish()
    }

    /// Builds the public kernel envelope without any alternate serialization path.
    #[must_use]
    pub fn envelope(
        &self,
        session_id: impl Into<String>,
        input_seq: u64,
        expected_state_version: u64,
        logical_ts_ns: i64,
    ) -> InputEnvelope {
        InputEnvelope {
            session_id: session_id.into(),
            input_seq,
            expected_state_version,
            logical_ts_ns,
            kind: self.kind().into(),
            payload: self.canonical_payload(),
        }
    }

    fn decode(kind: &str, bytes: &[u8]) -> Result<Self, FacadeError> {
        let mut reader = PayloadReader::new(bytes)?;
        let value = match kind {
            KIND_ORDER_SUBMIT => Self::SubmitOrder(SubmitOrderInput {
                request: read_new_order(&mut reader)?,
                quote: read_quote(&mut reader)?,
            }),
            KIND_ORDER_CANCEL => Self::CancelOrder {
                order_id: reader.u64()?,
            },
            KIND_ORDER_REPLACE => Self::ReplaceOrder(ReplaceOrderInput {
                order_id: reader.u64()?,
                replacement: ReplaceOrder {
                    quantity: QtyAtoms::new(reader.u64()?),
                    kind: read_order_kind(&mut reader)?,
                },
                quote: read_quote(&mut reader)?,
            }),
            KIND_EXECUTE_F0 => {
                let order_id = reader.u64()?;
                let bar = Bar {
                    event_seq: reader.u64()?,
                    open: PriceAtoms::new(reader.i64()?),
                    high: PriceAtoms::new(reader.i64()?),
                    low: PriceAtoms::new(reader.i64()?),
                    close: PriceAtoms::new(reader.i64()?),
                    base_volume: QtyAtoms::new(reader.u64()?),
                };
                let policy = match reader.text()?.as_str() {
                    "PESSIMISTIC" => IntrabarPolicy::Pessimistic,
                    "OPTIMISTIC" => IntrabarPolicy::Optimistic,
                    "SEEDED" => IntrabarPolicy::Seeded {
                        seed: reader.u64()?,
                    },
                    _ => return Err(FacadeError::new(FacadeErrorCode::InvalidPayload)),
                };
                Self::ExecuteF0 {
                    order_id,
                    bar,
                    config: F0Config {
                        intrabar_policy: policy,
                        market_slippage_atoms: reader.u64()?,
                    },
                }
            }
            KIND_EXECUTE_F1_QUOTE => Self::ExecuteF1Quote {
                order_id: reader.u64()?,
                quote: read_bbo(&mut reader)?,
                config: read_f1_config(&mut reader)?,
            },
            KIND_EXECUTE_F1_TRADE => Self::ExecuteF1Trade {
                order_id: reader.u64()?,
                trade: read_trade(&mut reader)?,
                eligible_after_event_seq: reader.u64()?,
                displayed_ahead: QtyAtoms::new(reader.u64()?),
                config: read_f1_config(&mut reader)?,
            },
            KIND_F2_SNAPSHOT => Self::F2Snapshot(L2Snapshot {
                sequence: reader.u64()?,
                bids: read_levels(&mut reader)?,
                asks: read_levels(&mut reader)?,
            }),
            KIND_F2_DELTA => Self::F2Delta(L2Delta {
                previous_sequence: reader.u64()?,
                sequence: reader.u64()?,
                side: match reader.text()?.as_str() {
                    "BID" => BookSide::Bid,
                    "ASK" => BookSide::Ask,
                    _ => return Err(FacadeError::new(FacadeErrorCode::InvalidPayload)),
                },
                price: PriceAtoms::new(reader.i64()?),
                quantity: QtyAtoms::new(reader.u64()?),
            }),
            KIND_EXECUTE_F2 => Self::ExecuteF2 {
                order_id: reader.u64()?,
                config: SweepConfig {
                    max_levels: usize::try_from(reader.u64()?)
                        .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidPayload))?,
                    max_quantity: read_optional_qty(&mut reader)?,
                },
            },
            KIND_SET_LEVERAGE => Self::SetLeverage {
                leverage: u8::try_from(reader.u64()?)
                    .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidPayload))?,
                equity: MoneyMinor::new(reader.i64()?),
                mark_price: PriceAtoms::new(reader.i64()?),
            },
            KIND_EVALUATE_RISK => Self::EvaluateRisk {
                equity: MoneyMinor::new(reader.i64()?),
                mark_price: PriceAtoms::new(reader.i64()?),
            },
            KIND_FUNDING => Self::Funding(ScheduledCashFlow {
                id: ScheduledEconomicId::new(reader.text()?, reader.text()?)
                    .map_err(FacadeError::from_economics)?,
                event_seq: 0,
                cash_delta: MoneyMinor::new(reader.i64()?),
            }),
            _ => return Err(FacadeError::new(FacadeErrorCode::UnsupportedInputKind)),
        };
        reader.finish()?;
        Ok(value)
    }
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

/// Versioned deterministic simulator facade.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorFacade {
    config: FacadeConfig,
    kernel: Kernel,
    last_logical_ts_ns: Option<i64>,
    market_event_seq: Option<u64>,
    orders: OrderState,
    position: Position,
    ledger: Ledger,
    economics: EconomicsState,
    risk: RiskState,
    f2_book: Option<L2Book>,
}

impl SimulatorFacade {
    /// Creates a pristine facade after validating all forgeable initial state.
    ///
    /// # Errors
    /// Returns a stable configuration/snapshot validation error without creating partial state.
    pub fn new(config: FacadeConfig, initial: FacadeInitialState) -> Result<Self, FacadeError> {
        config.validate()?;
        let position = Position::from_snapshot(initial.position).map_err(FacadeError::from_position)?;
        let ledger = Ledger::from_snapshot(initial.ledger).map_err(FacadeError::from_ledger)?;
        let f2_book = (config.execution_tier == ExecutionTier::F2).then(L2Book::new);
        Ok(Self {
            config,
            kernel: Kernel::new(),
            last_logical_ts_ns: None,
            market_event_seq: None,
            orders: OrderState::new(),
            position,
            ledger,
            economics: EconomicsState::new(),
            risk: RiskState::new(initial.leverage),
            f2_book,
        })
    }

    /// Restores complete deterministic continuation state and revalidates public snapshot parts.
    ///
    /// # Errors
    /// Returns [`FacadeErrorCode::InvalidSnapshot`] or a stable compatibility error without
    /// producing a partially restored facade.
    pub fn from_snapshot(snapshot: SimulatorSnapshot) -> Result<Self, FacadeError> {
        if snapshot.format_version != SNAPSHOT_FORMAT_VERSION {
            return Err(FacadeError::new(
                FacadeErrorCode::UnsupportedSnapshotVersion,
            ));
        }
        snapshot.config.validate()?;
        let kernel = Kernel::from_snapshot(snapshot.kernel).map_err(|_| {
            FacadeError::new(FacadeErrorCode::InvalidSnapshot)
        })?;
        let position = Position::from_snapshot(snapshot.position)
            .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
        let ledger = Ledger::from_snapshot(snapshot.ledger)
            .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
        let has_inputs = kernel.state_version() > 0;
        if has_inputs != snapshot.last_logical_ts_ns.is_some() {
            return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
        }
        let f2_consistent = match snapshot.config.execution_tier {
            ExecutionTier::F2 => snapshot.f2_book.is_some(),
            _ => snapshot.f2_book.is_none(),
        };
        if !f2_consistent {
            return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
        }
        Ok(Self {
            config: snapshot.config,
            kernel,
            last_logical_ts_ns: snapshot.last_logical_ts_ns,
            market_event_seq: snapshot.market_event_seq,
            orders: snapshot.orders,
            position,
            ledger,
            economics: snapshot.economics,
            risk: snapshot.risk,
            f2_book: snapshot.f2_book,
        })
    }

    /// Captures complete deterministic continuation state.
    #[must_use]
    pub fn snapshot(&self) -> SimulatorSnapshot {
        SimulatorSnapshot {
            format_version: SNAPSHOT_FORMAT_VERSION,
            config: self.config.clone(),
            last_logical_ts_ns: self.last_logical_ts_ns,
            market_event_seq: self.market_event_seq,
            kernel: self.kernel.snapshot(),
            orders: self.orders.clone(),
            position: self.position,
            ledger: self.ledger.snapshot(),
            economics: self.economics.clone(),
            risk: self.risk,
            f2_book: self.f2_book.clone(),
        }
    }

    /// Current optimistic state version for constructing the next input envelope.
    #[must_use]
    pub const fn state_version(&self) -> u64 {
        self.kernel.state_version()
    }

    /// Current authoritative position.
    #[must_use]
    pub const fn position(&self) -> Position {
        self.position
    }

    /// Current authoritative order collection.
    #[must_use]
    pub const fn orders(&self) -> &OrderState {
        &self.orders
    }

    /// Current balanced ledger.
    #[must_use]
    pub const fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// Current risk/leverage/liquidation state.
    #[must_use]
    pub const fn risk(&self) -> RiskState {
        self.risk
    }

    /// Current F2 book when the session is F2.
    #[must_use]
    pub const fn f2_book(&self) -> Option<&L2Book> {
        self.f2_book.as_ref()
    }

    /// Applies one public input atomically and returns deterministic typed domain events.
    ///
    /// The complete facade is cloned first. Kernel sequence/version/hash validation and all
    /// domain transitions occur on the clone, and real state is replaced only after dispatch
    /// succeeds. Consequently an invalid payload, unsupported model, accounting failure, or
    /// execution error cannot advance the hash chain or partially mutate economic state.
    ///
    /// # Errors
    /// Returns a stable [`FacadeErrorCode`] with no state mutation.
    pub fn apply(&mut self, input: &InputEnvelope) -> Result<Vec<DomainEvent>, FacadeError> {
        if input.session_id != self.config.session_id {
            return Err(FacadeError::new(FacadeErrorCode::SessionMismatch));
        }
        if self
            .last_logical_ts_ns
            .is_some_and(|last| input.logical_ts_ns < last)
        {
            return Err(FacadeError::new(FacadeErrorCode::LogicalTimeRegression));
        }
        let command = FacadeInput::decode(&input.kind, &input.payload)?;
        let mut next = self.clone();
        let cause = next.kernel.apply(input).map_err(FacadeError::from_kernel)?;
        let events = next.dispatch(command, &cause, input.logical_ts_ns)?;
        next.last_logical_ts_ns = Some(input.logical_ts_ns);
        *self = next;
        Ok(events)
    }

    fn dispatch(
        &mut self,
        mut command: FacadeInput,
        cause: &KernelEvent,
        logical_ts_ns: i64,
    ) -> Result<Vec<DomainEvent>, FacadeError> {
        let mut events = Vec::new();
        match &mut command {
            FacadeInput::SubmitOrder(input) => {
                if input.request.instrument_id != self.config.instrument_id {
                    return Err(FacadeError::new(FacadeErrorCode::InstrumentMismatch));
                }
                self.validate_submission_frontier(input.request.submitted_at_event_seq)?;
                let outcome = self
                    .orders
                    .submit(input.request.clone(), self.position.quantity_atoms, input.quote)
                    .map_err(FacadeError::from_order)?;
                let order = self
                    .orders
                    .get(outcome.order_id)
                    .ok_or_else(|| FacadeError::new(FacadeErrorCode::UnknownOrder))?
                    .clone();
                emit(&mut events, cause, DomainEventPayload::OrderSubmitted(order))?;
            }
            FacadeInput::CancelOrder { order_id } => {
                let id = self.resolve_order_id(*order_id)?;
                self.orders.cancel(id).map_err(FacadeError::from_order)?;
                emit(
                    &mut events,
                    cause,
                    DomainEventPayload::OrderCancelled { order_id: id },
                )?;
            }
            FacadeInput::ReplaceOrder(input) => {
                let id = self.resolve_order_id(input.order_id)?;
                self.orders
                    .replace(
                        id,
                        input.replacement,
                        self.position.quantity_atoms,
                        input.quote,
                    )
                    .map_err(FacadeError::from_order)?;
                let order = self
                    .orders
                    .get(id)
                    .ok_or_else(|| FacadeError::new(FacadeErrorCode::UnknownOrder))?
                    .clone();
                emit(&mut events, cause, DomainEventPayload::OrderReplaced(order))?;
            }
            FacadeInput::ExecuteF0 {
                order_id,
                bar,
                config,
            } => {
                self.require_tier(ExecutionTier::F0)?;
                self.observe_market_seq(bar.event_seq)?;
                let id = self.resolve_order_id(*order_id)?;
                let side = self.order_side(id)?;
                let outcome = execute_bar(&mut self.orders, id, *bar, *config)
                    .map_err(FacadeError::from_f0)?;
                let fills = outcome
                    .fill_price
                    .filter(|_| outcome.filled.get() > 0)
                    .map(|price| {
                        vec![ExecutionFill {
                            quantity: outcome.filled,
                            price,
                            liquidity_role: self.config.rules.f0_fee_role,
                        }]
                    })
                    .unwrap_or_default();
                let uncertainty = outcome
                    .uncertainty
                    .iter()
                    .map(|flag| match flag {
                        UncertaintyFlag::IntrabarAmbiguous => "INTRABAR_AMBIGUOUS".to_string(),
                    })
                    .collect();
                self.reconcile_execution(
                    cause,
                    id,
                    side,
                    fills,
                    outcome.status,
                    uncertainty,
                    &mut events,
                )?;
            }
            FacadeInput::ExecuteF1Quote {
                order_id,
                quote,
                config,
            } => {
                self.require_tier(ExecutionTier::F1)?;
                self.observe_market_seq(quote.event_seq)?;
                let id = self.resolve_order_id(*order_id)?;
                let side = self.order_side(id)?;
                let outcome = execute_on_quote(
                    &mut self.orders,
                    id,
                    *quote,
                    quote.event_seq,
                    logical_ts_ns,
                    *config,
                )
                .map_err(FacadeError::from_f1)?;
                let fills = match (outcome.fill_price, outcome.liquidity_role) {
                    (Some(price), Some(role)) if outcome.filled.get() > 0 => vec![ExecutionFill {
                        quantity: outcome.filled,
                        price,
                        liquidity_role: map_f1_role(role),
                    }],
                    _ => Vec::new(),
                };
                let uncertainty = map_f1_uncertainty(&outcome.uncertainty);
                self.reconcile_execution(
                    cause,
                    id,
                    side,
                    fills,
                    outcome.status,
                    uncertainty,
                    &mut events,
                )?;
            }
            FacadeInput::ExecuteF1Trade {
                order_id,
                trade,
                eligible_after_event_seq,
                displayed_ahead,
                config,
            } => {
                self.require_tier(ExecutionTier::F1)?;
                self.observe_market_seq(trade.event_seq)?;
                let id = self.resolve_order_id(*order_id)?;
                let order = self
                    .orders
                    .get(id)
                    .ok_or_else(|| FacadeError::new(FacadeErrorCode::UnknownOrder))?;
                if order.time_in_force != TimeInForce::Gtc
                    || !matches!(order.kind, OrderKind::Limit { .. })
                {
                    return Err(FacadeError::new(
                        FacadeErrorCode::UnsupportedOrderCombination,
                    ));
                }
                let side = order.side;
                let outcome = execute_resting_on_trade(
                    &mut self.orders,
                    id,
                    *trade,
                    trade.event_seq,
                    logical_ts_ns,
                    *eligible_after_event_seq,
                    *displayed_ahead,
                    &DisplayedAheadQueue,
                    *config,
                )
                .map_err(FacadeError::from_f1)?;
                let fills = outcome
                    .fill_price
                    .filter(|_| outcome.filled.get() > 0)
                    .map(|price| {
                        vec![ExecutionFill {
                            quantity: outcome.filled,
                            price,
                            liquidity_role: LiquidityRole::Maker,
                        }]
                    })
                    .unwrap_or_default();
                self.reconcile_execution(
                    cause,
                    id,
                    side,
                    fills,
                    outcome.status,
                    map_f1_uncertainty(&outcome.uncertainty),
                    &mut events,
                )?;
            }
            FacadeInput::F2Snapshot(snapshot) => {
                self.require_tier(ExecutionTier::F2)?;
                self.observe_market_seq(snapshot.sequence)?;
                self.f2_book_mut()?
                    .apply_snapshot(snapshot.clone())
                    .map_err(FacadeError::from_f2)?;
                emit(
                    &mut events,
                    cause,
                    DomainEventPayload::DepthSnapshotApplied {
                        sequence: snapshot.sequence,
                    },
                )?;
            }
            FacadeInput::F2Delta(delta) => {
                self.require_tier(ExecutionTier::F2)?;
                self.observe_market_seq(delta.sequence)?;
                self.f2_book_mut()?
                    .apply_delta(*delta)
                    .map_err(FacadeError::from_f2)?;
                emit(
                    &mut events,
                    cause,
                    DomainEventPayload::DepthDeltaApplied {
                        sequence: delta.sequence,
                    },
                )?;
            }
            FacadeInput::ExecuteF2 { order_id, config } => {
                self.require_tier(ExecutionTier::F2)?;
                let id = self.resolve_order_id(*order_id)?;
                let side = self.order_side(id)?;
                let mut book = self
                    .f2_book
                    .take()
                    .ok_or_else(|| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
                let outcome = execute_taker(&mut self.orders, &mut book, id, *config)
                    .map_err(FacadeError::from_f2)?;
                self.f2_book = Some(book);
                let fills = outcome
                    .levels
                    .iter()
                    .map(|level| ExecutionFill {
                        quantity: level.quantity,
                        price: level.price,
                        liquidity_role: LiquidityRole::Taker,
                    })
                    .collect();
                self.reconcile_execution(
                    cause,
                    id,
                    side,
                    fills,
                    outcome.status,
                    Vec::new(),
                    &mut events,
                )?;
            }
            FacadeInput::SetLeverage {
                leverage,
                equity,
                mark_price,
            } => {
                let requested = Leverage::new(*leverage).map_err(FacadeError::from_risk)?;
                let margin = self
                    .risk
                    .set_leverage(
                        requested,
                        *equity,
                        self.position,
                        &self.config.instrument_id,
                        &self.orders,
                        *mark_price,
                        self.config.rules.risk_profile,
                    )
                    .map_err(FacadeError::from_risk)?;
                emit(
                    &mut events,
                    cause,
                    DomainEventPayload::LeverageChanged {
                        leverage: requested.get(),
                        margin,
                    },
                )?;
            }
            FacadeInput::EvaluateRisk { equity, mark_price } => {
                let margin = margin_snapshot(
                    *equity,
                    self.position,
                    &self.config.instrument_id,
                    &self.orders,
                    *mark_price,
                    self.risk.leverage(),
                    self.config.rules.risk_profile,
                )
                .map_err(FacadeError::from_risk)?;
                let triggered = self.risk.evaluate_liquidation(self.position, margin);
                emit(
                    &mut events,
                    cause,
                    DomainEventPayload::RiskEvaluated {
                        margin,
                        liquidation_state: self.risk.liquidation_state(),
                        triggered,
                    },
                )?;
            }
            FacadeInput::Funding(flow) => {
                flow.event_seq = cause.event_seq;
                let posted = self
                    .economics
                    .post_funding(&mut self.ledger, flow.clone())
                    .map_err(FacadeError::from_economics)?;
                emit(
                    &mut events,
                    cause,
                    DomainEventPayload::FundingProcessed {
                        posted,
                        cash_delta: flow.cash_delta,
                    },
                )?;
            }
        }
        Ok(events)
    }

    fn reconcile_execution(
        &mut self,
        cause: &KernelEvent,
        order_id: OrderId,
        side: Side,
        fills: Vec<ExecutionFill>,
        status: OrderStatus,
        uncertainty: Vec<String>,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
        emit(
            events,
            cause,
            DomainEventPayload::Execution {
                order_id,
                fills: fills.clone(),
                status,
                uncertainty,
            },
        )?;
        if fills.is_empty() {
            return Ok(());
        }
        for fill in &fills {
            self.position
                .apply_fill(
                    match side {
                        Side::Buy => FillSide::Buy,
                        Side::Sell => FillSide::Sell,
                    },
                    fill.quantity,
                    fill.price,
                    self.config.rules.position_math,
                )
                .map_err(FacadeError::from_position)?;
            let rate = match fill.liquidity_role {
                LiquidityRole::Maker => self.config.rules.maker_fee_rate,
                LiquidityRole::Taker => self.config.rules.taker_fee_rate,
            };
            let fee = self
                .economics
                .post_execution_fee(
                    &mut self.ledger,
                    ExecutionFeeInput {
                        event_seq: cause.event_seq,
                        quantity: fill.quantity,
                        price: fill.price,
                        rate,
                        role: fill.liquidity_role,
                        math: self.config.rules.risk_profile.math,
                    },
                )
                .map_err(FacadeError::from_economics)?;
            emit(
                events,
                cause,
                DomainEventPayload::FeePosted {
                    role: fill.liquidity_role,
                    amount: fee,
                },
            )?;
        }
        emit(
            events,
            cause,
            DomainEventPayload::PositionChanged(self.position),
        )?;
        Ok(())
    }

    fn require_tier(&self, required: ExecutionTier) -> Result<(), FacadeError> {
        if self.config.execution_tier != required {
            return Err(FacadeError::new(FacadeErrorCode::ExecutionTierMismatch));
        }
        Ok(())
    }

    fn observe_market_seq(&mut self, sequence: u64) -> Result<(), FacadeError> {
        if self.market_event_seq.is_some_and(|current| sequence < current) {
            return Err(FacadeError::new(
                FacadeErrorCode::MarketSequenceRegression,
            ));
        }
        self.market_event_seq = Some(sequence);
        Ok(())
    }

    fn validate_submission_frontier(&self, submitted: u64) -> Result<(), FacadeError> {
        let expected = self.market_event_seq.unwrap_or(0);
        if submitted != expected {
            return Err(FacadeError::new(
                FacadeErrorCode::MarketSequenceRegression,
            ));
        }
        Ok(())
    }

    fn resolve_order_id(&self, raw: u64) -> Result<OrderId, FacadeError> {
        self.orders
            .iter()
            .find(|order| order.id.get() == raw)
            .map(|order| order.id)
            .ok_or_else(|| FacadeError::new(FacadeErrorCode::UnknownOrder))
    }

    fn order_side(&self, id: OrderId) -> Result<Side, FacadeError> {
        self.orders
            .get(id)
            .map(|order| order.side)
            .ok_or_else(|| FacadeError::new(FacadeErrorCode::UnknownOrder))
    }

    fn f2_book_mut(&mut self) -> Result<&mut L2Book, FacadeError> {
        self.f2_book
            .as_mut()
            .ok_or_else(|| FacadeError::new(FacadeErrorCode::InvalidSnapshot))
    }
}

fn emit(
    events: &mut Vec<DomainEvent>,
    cause: &KernelEvent,
    payload: DomainEventPayload,
) -> Result<(), FacadeError> {
    let ordinal = u32::try_from(events.len())
        .map_err(|_| FacadeError::new(FacadeErrorCode::CounterOverflow))?;
    events.push(DomainEvent {
        cause: cause.clone(),
        ordinal,
        payload,
    });
    Ok(())
}

fn map_f1_role(role: F1LiquidityRole) -> LiquidityRole {
    match role {
        F1LiquidityRole::Maker => LiquidityRole::Maker,
        F1LiquidityRole::Taker => LiquidityRole::Taker,
    }
}

fn map_f1_uncertainty(flags: &[F1Uncertainty]) -> Vec<String> {
    flags
        .iter()
        .map(|flag| match flag {
            F1Uncertainty::MakerQueueApproximation => "MAKER_QUEUE_APPROXIMATION".to_string(),
        })
        .collect()
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

fn write_new_order(writer: &mut CanonicalWriter, request: &NewOrder) {
    writer.text(&request.client_order_id);
    writer.text(&request.instrument_id);
    writer.text(match request.side {
        Side::Buy => "BUY",
        Side::Sell => "SELL",
    });
    writer.u64(request.quantity.get());
    write_order_kind(writer, request.kind);
    writer.text(match request.time_in_force {
        TimeInForce::Gtc => "GTC",
        TimeInForce::Ioc => "IOC",
        TimeInForce::Fok => "FOK",
    });
    write_bool(writer, request.reduce_only);
    write_bool(writer, request.post_only);
    write_bool(writer, request.marketable_only);
    writer.u64(request.submitted_at_event_seq);
}

fn read_new_order(reader: &mut PayloadReader<'_>) -> Result<NewOrder, FacadeError> {
    Ok(NewOrder {
        client_order_id: reader.text()?,
        instrument_id: reader.text()?,
        side: match reader.text()?.as_str() {
            "BUY" => Side::Buy,
            "SELL" => Side::Sell,
            _ => return Err(FacadeError::new(FacadeErrorCode::InvalidPayload)),
        },
        quantity: QtyAtoms::new(reader.u64()?),
        kind: read_order_kind(reader)?,
        time_in_force: match reader.text()?.as_str() {
            "GTC" => TimeInForce::Gtc,
            "IOC" => TimeInForce::Ioc,
            "FOK" => TimeInForce::Fok,
            _ => return Err(FacadeError::new(FacadeErrorCode::InvalidPayload)),
        },
        reduce_only: reader.boolean()?,
        post_only: reader.boolean()?,
        marketable_only: reader.boolean()?,
        submitted_at_event_seq: reader.u64()?,
    })
}

fn write_order_kind(writer: &mut CanonicalWriter, kind: OrderKind) {
    match kind {
        OrderKind::Market => writer.text("MARKET"),
        OrderKind::Limit { limit_price } => {
            writer.text("LIMIT");
            writer.i64(limit_price.get());
        }
        OrderKind::StopMarket { stop_price } => {
            writer.text("STOP_MARKET");
            writer.i64(stop_price.get());
        }
        OrderKind::StopLimit {
            stop_price,
            limit_price,
        } => {
            writer.text("STOP_LIMIT");
            writer.i64(stop_price.get());
            writer.i64(limit_price.get());
        }
    }
}

fn read_order_kind(reader: &mut PayloadReader<'_>) -> Result<OrderKind, FacadeError> {
    match reader.text()?.as_str() {
        "MARKET" => Ok(OrderKind::Market),
        "LIMIT" => Ok(OrderKind::Limit {
            limit_price: PriceAtoms::new(reader.i64()?),
        }),
        "STOP_MARKET" => Ok(OrderKind::StopMarket {
            stop_price: PriceAtoms::new(reader.i64()?),
        }),
        "STOP_LIMIT" => Ok(OrderKind::StopLimit {
            stop_price: PriceAtoms::new(reader.i64()?),
            limit_price: PriceAtoms::new(reader.i64()?),
        }),
        _ => Err(FacadeError::new(FacadeErrorCode::InvalidPayload)),
    }
}

fn write_quote(writer: &mut CanonicalWriter, quote: Option<TopOfBook>) {
    write_bool(writer, quote.is_some());
    if let Some(quote) = quote {
        writer.i64(quote.bid.get());
        writer.i64(quote.ask.get());
    }
}

fn read_quote(reader: &mut PayloadReader<'_>) -> Result<Option<TopOfBook>, FacadeError> {
    if !reader.boolean()? {
        return Ok(None);
    }
    Ok(Some(TopOfBook {
        bid: PriceAtoms::new(reader.i64()?),
        ask: PriceAtoms::new(reader.i64()?),
    }))
}

fn write_bbo(writer: &mut CanonicalWriter, quote: BboQuote) {
    writer.u64(quote.event_seq);
    writer.i64(quote.event_time_ns);
    writer.i64(quote.bid.get());
    writer.u64(quote.bid_size.get());
    writer.i64(quote.ask.get());
    writer.u64(quote.ask_size.get());
}

fn read_bbo(reader: &mut PayloadReader<'_>) -> Result<BboQuote, FacadeError> {
    Ok(BboQuote {
        event_seq: reader.u64()?,
        event_time_ns: reader.i64()?,
        bid: PriceAtoms::new(reader.i64()?),
        bid_size: QtyAtoms::new(reader.u64()?),
        ask: PriceAtoms::new(reader.i64()?),
        ask_size: QtyAtoms::new(reader.u64()?),
    })
}

fn write_trade(writer: &mut CanonicalWriter, trade: TradePrint) {
    writer.u64(trade.event_seq);
    writer.i64(trade.event_time_ns);
    writer.i64(trade.price.get());
    writer.u64(trade.quantity.get());
}

fn read_trade(reader: &mut PayloadReader<'_>) -> Result<TradePrint, FacadeError> {
    Ok(TradePrint {
        event_seq: reader.u64()?,
        event_time_ns: reader.i64()?,
        price: PriceAtoms::new(reader.i64()?),
        quantity: QtyAtoms::new(reader.u64()?),
    })
}

fn write_f1_config(writer: &mut CanonicalWriter, config: F1Config) {
    writer.u64(config.max_quote_age_ns);
    writer.u64(config.max_trade_age_ns);
    write_optional_qty(writer, config.max_taker_fill);
    write_optional_qty(writer, config.max_maker_fill);
}

fn read_f1_config(reader: &mut PayloadReader<'_>) -> Result<F1Config, FacadeError> {
    Ok(F1Config {
        max_quote_age_ns: reader.u64()?,
        max_trade_age_ns: reader.u64()?,
        max_taker_fill: read_optional_qty(reader)?,
        max_maker_fill: read_optional_qty(reader)?,
    })
}

fn write_levels(writer: &mut CanonicalWriter, levels: &[DepthLevel]) {
    writer.u64(u64::try_from(levels.len()).unwrap_or(u64::MAX));
    for level in levels {
        writer.i64(level.price.get());
        writer.u64(level.quantity.get());
    }
}

fn read_levels(reader: &mut PayloadReader<'_>) -> Result<Vec<DepthLevel>, FacadeError> {
    let count = usize::try_from(reader.u64()?)
        .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidPayload))?;
    if count > MAX_DEPTH_LEVELS_PER_INPUT {
        return Err(FacadeError::new(FacadeErrorCode::InvalidPayload));
    }
    let mut levels = Vec::with_capacity(count);
    for _ in 0..count {
        levels.push(DepthLevel {
            price: PriceAtoms::new(reader.i64()?),
            quantity: QtyAtoms::new(reader.u64()?),
        });
    }
    Ok(levels)
}

fn write_optional_qty(writer: &mut CanonicalWriter, value: Option<QtyAtoms>) {
    write_bool(writer, value.is_some());
    if let Some(value) = value {
        writer.u64(value.get());
    }
}

fn read_optional_qty(reader: &mut PayloadReader<'_>) -> Result<Option<QtyAtoms>, FacadeError> {
    if reader.boolean()? {
        Ok(Some(QtyAtoms::new(reader.u64()?)))
    } else {
        Ok(None)
    }
}

fn write_bool(writer: &mut CanonicalWriter, value: bool) {
    writer.u64(u64::from(value));
}

struct PayloadReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> PayloadReader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, FacadeError> {
        if !bytes.starts_with(PAYLOAD_TAG) {
            return Err(FacadeError::new(FacadeErrorCode::InvalidPayload));
        }
        Ok(Self {
            bytes,
            offset: PAYLOAD_TAG.len(),
        })
    }

    fn u64(&mut self) -> Result<u64, FacadeError> {
        let raw = self.take(8)?;
        Ok(u64::from_be_bytes(raw.try_into().map_err(|_| {
            FacadeError::new(FacadeErrorCode::InvalidPayload)
        })?))
    }

    fn i64(&mut self) -> Result<i64, FacadeError> {
        let raw = self.take(8)?;
        Ok(i64::from_be_bytes(raw.try_into().map_err(|_| {
            FacadeError::new(FacadeErrorCode::InvalidPayload)
        })?))
    }

    fn boolean(&mut self) -> Result<bool, FacadeError> {
        match self.u64()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(FacadeError::new(FacadeErrorCode::InvalidPayload)),
        }
    }

    fn text(&mut self) -> Result<String, FacadeError> {
        let length = usize::try_from(self.u64()?)
            .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidPayload))?;
        if length > MAX_TEXT_BYTES {
            return Err(FacadeError::new(FacadeErrorCode::InvalidPayload));
        }
        let raw = self.take(length)?;
        core::str::from_utf8(raw)
            .map(str::to_owned)
            .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidPayload))
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], FacadeError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| FacadeError::new(FacadeErrorCode::InvalidPayload))?;
        let result = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| FacadeError::new(FacadeErrorCode::InvalidPayload))?;
        self.offset = end;
        Ok(result)
    }

    fn finish(self) -> Result<(), FacadeError> {
        if self.offset != self.bytes.len() {
            return Err(FacadeError::new(FacadeErrorCode::InvalidPayload));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economics::EconomicsMath;
    use crate::hash::ZERO_HASH;
    use crate::ledger::{LedgerAccount, Posting};
    use crate::numeric::{DecimalScale, Rounding};

    fn position_math() -> PositionMath {
        PositionMath {
            contract_multiplier_atoms: 1,
            qty_scale: DecimalScale::new(0).unwrap(),
            price_scale: DecimalScale::new(0).unwrap(),
            multiplier_scale: DecimalScale::new(0).unwrap(),
            settlement_scale: DecimalScale::new(0).unwrap(),
            rounding: Rounding::TowardZero,
        }
    }

    fn risk_profile() -> RiskProfile {
        let math = position_math();
        RiskProfile {
            maintenance_rate: RatePpb::new(250_000_000),
            math: EconomicsMath {
                contract_multiplier_atoms: math.contract_multiplier_atoms,
                qty_scale: math.qty_scale,
                price_scale: math.price_scale,
                multiplier_scale: math.multiplier_scale,
                settlement_scale: math.settlement_scale,
                rounding: math.rounding,
            },
        }
    }

    fn config(tier: ExecutionTier) -> FacadeConfig {
        FacadeConfig {
            api_version: FACADE_API_VERSION,
            session_id: "session-1".into(),
            instrument_id: "SYNTH".into(),
            execution_tier: tier,
            rules: FacadeRules {
                position_math: position_math(),
                risk_profile: risk_profile(),
                f0_fee_role: LiquidityRole::Taker,
                maker_fee_rate: RatePpb::new(0),
                taker_fee_rate: RatePpb::new(10_000_000),
            },
        }
    }

    fn initial() -> FacadeInitialState {
        FacadeInitialState {
            position: Position::flat(),
            ledger: LedgerSnapshot {
                next_transaction_id: 0,
                balances: Vec::new(),
            },
            leverage: Leverage::new(2).unwrap(),
        }
    }

    fn apply(facade: &mut SimulatorFacade, input: FacadeInput, logical: i64) -> Vec<DomainEvent> {
        let sequence = facade.state_version();
        facade
            .apply(&input.envelope("session-1", sequence, sequence, logical))
            .unwrap()
    }

    fn market_order(submitted_at_event_seq: u64) -> NewOrder {
        NewOrder {
            client_order_id: format!("order-{submitted_at_event_seq}"),
            instrument_id: "SYNTH".into(),
            side: Side::Buy,
            quantity: QtyAtoms::new(2),
            kind: OrderKind::Market,
            time_in_force: TimeInForce::Gtc,
            reduce_only: false,
            post_only: false,
            marketable_only: false,
            submitted_at_event_seq,
        }
    }

    fn submit(facade: &mut SimulatorFacade, market_seq: u64, logical: i64) -> OrderId {
        let events = apply(
            facade,
            FacadeInput::SubmitOrder(SubmitOrderInput {
                request: market_order(market_seq),
                quote: None,
            }),
            logical,
        );
        match &events[0].payload {
            DomainEventPayload::OrderSubmitted(order) => order.id,
            _ => panic!("expected submitted order"),
        }
    }

    #[test]
    fn canonical_payload_round_trip_and_trailing_bytes_fail_closed() {
        let input = FacadeInput::SubmitOrder(SubmitOrderInput {
            request: market_order(0),
            quote: None,
        });
        let payload = input.canonical_payload();
        assert_eq!(FacadeInput::decode(input.kind(), &payload), Ok(input.clone()));
        let mut trailing = payload;
        trailing.push(0);
        assert_eq!(
            FacadeInput::decode(input.kind(), &trailing),
            Err(FacadeError::new(FacadeErrorCode::InvalidPayload))
        );
    }

    #[test]
    fn unsupported_tier_fails_with_stable_code() {
        let error = SimulatorFacade::new(config(ExecutionTier::F3), initial()).unwrap_err();
        assert_eq!(error.code, FacadeErrorCode::UnsupportedExecutionTier);
        assert_eq!(ExecutionModelRegistry.supported(), [ExecutionTier::F0, ExecutionTier::F1, ExecutionTier::F2]);
    }

    #[test]
    fn invalid_domain_input_does_not_advance_kernel_or_state() {
        let mut facade = SimulatorFacade::new(config(ExecutionTier::F0), initial()).unwrap();
        let before = facade.snapshot();
        let invalid = FacadeInput::SubmitOrder(SubmitOrderInput {
            request: NewOrder {
                instrument_id: "OTHER".into(),
                ..market_order(0)
            },
            quote: None,
        });
        let envelope = invalid.envelope("session-1", 0, 0, 1);
        assert_eq!(
            facade.apply(&envelope),
            Err(FacadeError::new(FacadeErrorCode::InstrumentMismatch))
        );
        assert_eq!(facade.snapshot(), before);
        assert_eq!(facade.snapshot().kernel.current_event_hash, ZERO_HASH);
    }

    #[test]
    fn f0_execution_updates_order_position_and_exact_fee() {
        let mut facade = SimulatorFacade::new(config(ExecutionTier::F0), initial()).unwrap();
        let id = submit(&mut facade, 0, 1);
        let events = apply(
            &mut facade,
            FacadeInput::ExecuteF0 {
                order_id: id.get(),
                bar: Bar {
                    event_seq: 1,
                    open: PriceAtoms::new(100),
                    high: PriceAtoms::new(105),
                    low: PriceAtoms::new(95),
                    close: PriceAtoms::new(101),
                    base_volume: QtyAtoms::new(100),
                },
                config: F0Config::default(),
            },
            2,
        );
        assert!(matches!(events[0].payload, DomainEventPayload::Execution { .. }));
        assert!(events.iter().any(|event| matches!(event.payload, DomainEventPayload::FeePosted { .. })));
        assert_eq!(facade.position().quantity_atoms, 2);
        assert_eq!(facade.position().average_entry_price, Some(PriceAtoms::new(100)));
        assert_eq!(facade.ledger().balance(LedgerAccount::Fees), MoneyMinor::new(2));
        assert_eq!(facade.ledger().balance(LedgerAccount::Cash), MoneyMinor::new(-2));
    }

    #[test]
    fn restored_f0_run_matches_uninterrupted_events_and_hashes() {
        let mut uninterrupted = SimulatorFacade::new(config(ExecutionTier::F0), initial()).unwrap();
        let id = submit(&mut uninterrupted, 0, 1);
        let snapshot = uninterrupted.snapshot();
        let execution = FacadeInput::ExecuteF0 {
            order_id: id.get(),
            bar: Bar {
                event_seq: 1,
                open: PriceAtoms::new(100),
                high: PriceAtoms::new(100),
                low: PriceAtoms::new(100),
                close: PriceAtoms::new(100),
                base_volume: QtyAtoms::new(5),
            },
            config: F0Config::default(),
        };
        let expected = apply(&mut uninterrupted, execution.clone(), 2);

        let mut restored = SimulatorFacade::from_snapshot(snapshot).unwrap();
        let actual = apply(&mut restored, execution, 2);
        assert_eq!(actual, expected);
        assert_eq!(restored.snapshot(), uninterrupted.snapshot());
        assert_eq!(
            restored.snapshot().kernel.current_event_hash,
            uninterrupted.snapshot().kernel.current_event_hash
        );
    }

    #[test]
    fn f1_resting_trade_rejects_market_order_with_stable_code_atomically() {
        let mut facade = SimulatorFacade::new(config(ExecutionTier::F1), initial()).unwrap();
        let id = submit(&mut facade, 0, 1);
        let before = facade.snapshot();
        let command = FacadeInput::ExecuteF1Trade {
            order_id: id.get(),
            trade: TradePrint {
                event_seq: 1,
                event_time_ns: 2,
                price: PriceAtoms::new(100),
                quantity: QtyAtoms::new(2),
            },
            eligible_after_event_seq: 0,
            displayed_ahead: QtyAtoms::new(0),
            config: F1Config::default(),
        };
        let envelope = command.envelope("session-1", 1, 1, 2);
        assert_eq!(
            facade.apply(&envelope),
            Err(FacadeError::new(
                FacadeErrorCode::UnsupportedOrderCombination
            ))
        );
        assert_eq!(facade.snapshot(), before);
    }

    #[test]
    fn f2_snapshot_sweep_and_restore_preserve_depth_continuity() {
        let mut facade = SimulatorFacade::new(config(ExecutionTier::F2), initial()).unwrap();
        apply(
            &mut facade,
            FacadeInput::F2Snapshot(L2Snapshot {
                sequence: 5,
                bids: vec![DepthLevel {
                    price: PriceAtoms::new(99),
                    quantity: QtyAtoms::new(5),
                }],
                asks: vec![DepthLevel {
                    price: PriceAtoms::new(101),
                    quantity: QtyAtoms::new(5),
                }],
            }),
            1,
        );
        let id = submit(&mut facade, 5, 2);
        let snapshot = facade.snapshot();
        let execution = FacadeInput::ExecuteF2 {
            order_id: id.get(),
            config: SweepConfig::default(),
        };
        let expected = apply(&mut facade, execution.clone(), 3);
        let mut restored = SimulatorFacade::from_snapshot(snapshot).unwrap();
        let actual = apply(&mut restored, execution, 3);
        assert_eq!(actual, expected);
        assert_eq!(restored.snapshot(), facade.snapshot());
    }

    #[test]
    fn funding_idempotency_survives_snapshot_restore() {
        let mut facade = SimulatorFacade::new(config(ExecutionTier::F0), initial()).unwrap();
        let funding = FacadeInput::Funding(ScheduledCashFlow {
            id: ScheduledEconomicId::new("provider", "funding-1").unwrap(),
            event_seq: 999,
            cash_delta: MoneyMinor::new(-7),
        });
        let first = apply(&mut facade, funding.clone(), 1);
        assert!(matches!(
            first[0].payload,
            DomainEventPayload::FundingProcessed { posted: true, .. }
        ));
        let snapshot = facade.snapshot();
        let mut restored = SimulatorFacade::from_snapshot(snapshot).unwrap();
        let second = apply(&mut restored, funding, 2);
        assert!(matches!(
            second[0].payload,
            DomainEventPayload::FundingProcessed { posted: false, .. }
        ));
    }

    #[test]
    fn forged_invalid_snapshot_fails_closed() {
        let facade = SimulatorFacade::new(config(ExecutionTier::F0), initial()).unwrap();
        let mut snapshot = facade.snapshot();
        snapshot.position = Position {
            quantity_atoms: 1,
            average_entry_price: None,
            realized_pnl: MoneyMinor::new(0),
        };
        assert_eq!(
            SimulatorFacade::from_snapshot(snapshot),
            Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot))
        );
    }

    #[test]
    fn risk_and_funding_paths_remain_exact_and_versioned() {
        let mut facade = SimulatorFacade::new(config(ExecutionTier::F0), initial()).unwrap();
        let leverage = apply(
            &mut facade,
            FacadeInput::SetLeverage {
                leverage: 5,
                equity: MoneyMinor::new(1_000),
                mark_price: PriceAtoms::new(100),
            },
            1,
        );
        assert!(matches!(
            leverage[0].payload,
            DomainEventPayload::LeverageChanged { leverage: 5, .. }
        ));
        let risk = apply(
            &mut facade,
            FacadeInput::EvaluateRisk {
                equity: MoneyMinor::new(0),
                mark_price: PriceAtoms::new(100),
            },
            2,
        );
        assert!(matches!(
            risk[0].payload,
            DomainEventPayload::RiskEvaluated { .. }
        ));
        assert_eq!(facade.snapshot().config.api_version, FACADE_API_VERSION);
    }

    #[test]
    fn ledger_snapshot_in_test_is_balanced() {
        let snapshot = LedgerSnapshot {
            next_transaction_id: 2,
            balances: vec![
                (LedgerAccount::Cash, MoneyMinor::new(10)),
                (LedgerAccount::Fees, MoneyMinor::new(-10)),
            ],
        };
        assert!(Ledger::from_snapshot(snapshot).is_ok());
        let postings = [
            Posting {
                account: LedgerAccount::Cash,
                amount: MoneyMinor::new(1),
            },
            Posting {
                account: LedgerAccount::Fees,
                amount: MoneyMinor::new(-1),
            },
        ];
        assert_eq!(postings.len(), 2);
    }
}
