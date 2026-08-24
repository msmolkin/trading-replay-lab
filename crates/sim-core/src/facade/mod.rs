//! Versioned public simulator facade over authoritative domain state.

mod codec;
mod types;

pub use types::{
    DomainEvent, DomainEventPayload, ExecutionFill, ExecutionModelRegistry, ExecutionTier,
    FACADE_API_VERSION, FacadeConfig, FacadeError, FacadeErrorCode, FacadeInitialState,
    FacadeInput, FacadeRules, FundingInput, ReplaceOrderInput, SubmitOrderInput,
};

use crate::economics::{EconomicsState, ExecutionFeeInput, LiquidityRole, ScheduledCashFlow};
use crate::execution::f0::{UncertaintyFlag, execute_bar};
use crate::execution::f1::{
    DisplayedAheadQueue, F1LiquidityRole, F1Uncertainty, execute_on_quote, execute_resting_on_trade,
};
use crate::execution::f2::{L2Book, execute_taker};
use crate::kernel::{InputEnvelope, Kernel, KernelEvent};
use crate::ledger::Ledger;
use crate::orders::{OrderId, OrderKind, OrderState, OrderStatus, Side, TimeInForce};
use crate::positions::{FillSide, Position};
use crate::risk::{RiskState, margin_snapshot};
use crate::snapshot::{SNAPSHOT_FORMAT_VERSION, SimulatorSnapshot};

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
        let position =
            Position::from_snapshot(initial.position).map_err(FacadeError::from_position)?;
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
        let kernel = Kernel::from_snapshot(snapshot.kernel)
            .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
        let position = Position::from_snapshot(snapshot.position)
            .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
        let ledger = Ledger::from_snapshot(snapshot.ledger)
            .map_err(|_| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
        let has_inputs = kernel.state_version() > 0;
        if has_inputs != snapshot.last_logical_ts_ns.is_some() {
            return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
        }
        if snapshot
            .orders
            .iter()
            .any(|order| order.instrument_id != snapshot.config.instrument_id)
        {
            return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
        }
        validate_f2_snapshot_state(
            snapshot.config.execution_tier,
            snapshot.market_event_seq,
            snapshot.f2_book.as_ref(),
        )?;
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
        command: FacadeInput,
        cause: &KernelEvent,
        logical_ts_ns: i64,
    ) -> Result<Vec<DomainEvent>, FacadeError> {
        let mut events = Vec::new();
        match command {
            FacadeInput::SubmitOrder(input) => {
                if input.request.instrument_id != self.config.instrument_id {
                    return Err(FacadeError::new(FacadeErrorCode::InstrumentMismatch));
                }
                self.validate_submission_frontier(input.request.submitted_at_event_seq)?;
                let outcome = self
                    .orders
                    .submit(input.request, self.position.quantity_atoms, input.quote)
                    .map_err(FacadeError::from_order)?;
                let order = self
                    .orders
                    .get(outcome.order_id)
                    .ok_or_else(|| FacadeError::new(FacadeErrorCode::UnknownOrder))?
                    .clone();
                emit(
                    &mut events,
                    cause,
                    DomainEventPayload::OrderSubmitted(order),
                )?;
            }
            FacadeInput::CancelOrder { order_id } => {
                let id = self.resolve_order_id(order_id)?;
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
                let id = self.resolve_order_id(order_id)?;
                let side = self.order_side(id)?;
                let outcome =
                    execute_bar(&mut self.orders, id, bar, config).map_err(FacadeError::from_f0)?;
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
                let id = self.resolve_order_id(order_id)?;
                let side = self.order_side(id)?;
                let outcome = execute_on_quote(
                    &mut self.orders,
                    id,
                    quote,
                    quote.event_seq,
                    logical_ts_ns,
                    config,
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
            FacadeInput::ExecuteF1Trade {
                order_id,
                trade,
                eligible_after_event_seq,
                displayed_ahead,
                config,
            } => {
                self.require_tier(ExecutionTier::F1)?;
                self.observe_market_seq(trade.event_seq)?;
                let id = self.resolve_order_id(order_id)?;
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
                    trade,
                    trade.event_seq,
                    logical_ts_ns,
                    eligible_after_event_seq,
                    displayed_ahead,
                    &DisplayedAheadQueue,
                    config,
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
                let sequence = snapshot.sequence;
                self.f2_book_mut()?
                    .apply_snapshot(snapshot)
                    .map_err(FacadeError::from_f2)?;
                emit(
                    &mut events,
                    cause,
                    DomainEventPayload::DepthSnapshotApplied { sequence },
                )?;
            }
            FacadeInput::F2Delta(delta) => {
                self.require_tier(ExecutionTier::F2)?;
                self.observe_market_seq(delta.sequence)?;
                self.f2_book_mut()?
                    .apply_delta(delta)
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
                let id = self.resolve_order_id(order_id)?;
                let side = self.order_side(id)?;
                let mut book = self
                    .f2_book
                    .take()
                    .ok_or_else(|| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
                let outcome = execute_taker(&mut self.orders, &mut book, id, config)
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
                let requested =
                    crate::risk::Leverage::new(leverage).map_err(FacadeError::from_risk)?;
                let margin = self
                    .risk
                    .set_leverage(
                        requested,
                        equity,
                        self.position,
                        &self.config.instrument_id,
                        &self.orders,
                        mark_price,
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
                    equity,
                    self.position,
                    &self.config.instrument_id,
                    &self.orders,
                    mark_price,
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
            FacadeInput::Funding(input) => {
                let posted = self
                    .economics
                    .post_funding(
                        &mut self.ledger,
                        ScheduledCashFlow {
                            id: input.id,
                            event_seq: cause.event_seq,
                            cash_delta: input.cash_delta,
                        },
                    )
                    .map_err(FacadeError::from_economics)?;
                emit(
                    &mut events,
                    cause,
                    DomainEventPayload::FundingProcessed {
                        posted,
                        cash_delta: input.cash_delta,
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
        if self
            .market_event_seq
            .is_some_and(|current| sequence < current)
        {
            return Err(FacadeError::new(FacadeErrorCode::MarketSequenceRegression));
        }
        self.market_event_seq = Some(sequence);
        Ok(())
    }

    fn validate_submission_frontier(&self, submitted: u64) -> Result<(), FacadeError> {
        let expected = self.market_event_seq.unwrap_or(0);
        if submitted != expected {
            return Err(FacadeError::new(FacadeErrorCode::MarketSequenceRegression));
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

fn validate_f2_snapshot_state(
    tier: ExecutionTier,
    market_event_seq: Option<u64>,
    book: Option<&L2Book>,
) -> Result<(), FacadeError> {
    match (tier, book) {
        (ExecutionTier::F2, Some(book)) => {
            if book.sequence() != market_event_seq
                || book.is_enabled() != market_event_seq.is_some()
            {
                return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
            }
        }
        (ExecutionTier::F2, None) => {
            return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot));
        }
        (_, Some(_)) => return Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot)),
        (_, None) => {}
    }
    Ok(())
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

const fn map_f1_role(role: F1LiquidityRole) -> LiquidityRole {
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

#[cfg(test)]
mod tests;
