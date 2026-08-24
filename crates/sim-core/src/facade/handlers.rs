//! Typed facade transition handlers kept small enough for independent audit.

use crate::economics::{ExecutionFeeInput, LiquidityRole, ScheduledCashFlow};
use crate::execution::f0::{Bar, F0Config, UncertaintyFlag, execute_bar};
use crate::execution::f1::{
    BboQuote, DisplayedAheadQueue, F1Config, F1LiquidityRole, F1Uncertainty, TradePrint,
    execute_on_quote, execute_resting_on_trade,
};
use crate::execution::f2::{L2Delta, L2Snapshot, SweepConfig, execute_taker};
use crate::kernel::KernelEvent;
use crate::numeric::{MoneyMinor, PriceAtoms, QtyAtoms};
use crate::orders::{OrderId, OrderKind, OrderStatus, Side, TimeInForce};
use crate::positions::FillSide;
use crate::risk::{Leverage, margin_snapshot};

use super::{
    DomainEvent, DomainEventPayload, ExecutionFill, ExecutionTier, FacadeError, FacadeErrorCode,
    FacadeInput, FundingInput, ReplaceOrderInput, SimulatorFacade, SubmitOrderInput,
};

#[derive(Clone, Copy)]
struct F1TradeRequest {
    order_id: u64,
    trade: TradePrint,
    eligible_after_event_seq: u64,
    displayed_ahead: QtyAtoms,
    config: F1Config,
}

struct ExecutionReconciliation {
    order_id: OrderId,
    side: Side,
    fills: Vec<ExecutionFill>,
    status: OrderStatus,
    uncertainty: Vec<String>,
}

impl SimulatorFacade {
    pub(super) fn dispatch(
        &mut self,
        command: FacadeInput,
        cause: &KernelEvent,
        logical_ts_ns: i64,
    ) -> Result<Vec<DomainEvent>, FacadeError> {
        let mut events = Vec::new();
        match command {
            FacadeInput::SubmitOrder(input) => self.handle_submit(input, cause, &mut events)?,
            FacadeInput::CancelOrder { order_id } => {
                self.handle_cancel(order_id, cause, &mut events)?;
            }
            FacadeInput::ReplaceOrder(input) => self.handle_replace(input, cause, &mut events)?,
            FacadeInput::ExecuteF0 {
                order_id,
                bar,
                config,
            } => self.handle_f0(order_id, bar, config, cause, &mut events)?,
            FacadeInput::ExecuteF1Quote {
                order_id,
                quote,
                config,
            } => {
                self.handle_f1_quote(order_id, quote, config, cause, logical_ts_ns, &mut events)?;
            }
            FacadeInput::ExecuteF1Trade {
                order_id,
                trade,
                eligible_after_event_seq,
                displayed_ahead,
                config,
            } => self.handle_f1_trade(
                F1TradeRequest {
                    order_id,
                    trade,
                    eligible_after_event_seq,
                    displayed_ahead,
                    config,
                },
                cause,
                logical_ts_ns,
                &mut events,
            )?,
            FacadeInput::F2Snapshot(snapshot) => {
                self.handle_f2_snapshot(snapshot, cause, &mut events)?;
            }
            FacadeInput::F2Delta(delta) => self.handle_f2_delta(delta, cause, &mut events)?,
            FacadeInput::ExecuteF2 { order_id, config } => {
                self.handle_f2_execution(order_id, config, cause, &mut events)?;
            }
            FacadeInput::SetLeverage {
                leverage,
                equity,
                mark_price,
            } => self.handle_leverage(leverage, equity, mark_price, cause, &mut events)?,
            FacadeInput::EvaluateRisk { equity, mark_price } => {
                self.handle_risk(equity, mark_price, cause, &mut events)?;
            }
            FacadeInput::Funding(input) => self.handle_funding(input, cause, &mut events)?,
        }
        Ok(events)
    }

    fn handle_submit(
        &mut self,
        input: SubmitOrderInput,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
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
        emit(events, cause, DomainEventPayload::OrderSubmitted(order))
    }

    fn handle_cancel(
        &mut self,
        order_id: u64,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
        let id = self.resolve_order_id(order_id)?;
        self.orders.cancel(id).map_err(FacadeError::from_order)?;
        emit(
            events,
            cause,
            DomainEventPayload::OrderCancelled { order_id: id },
        )
    }

    fn handle_replace(
        &mut self,
        input: ReplaceOrderInput,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
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
        emit(events, cause, DomainEventPayload::OrderReplaced(order))
    }

    fn handle_f0(
        &mut self,
        order_id: u64,
        bar: Bar,
        config: F0Config,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
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
            events,
            ExecutionReconciliation {
                order_id: id,
                side,
                fills,
                status: outcome.status,
                uncertainty,
            },
        )
    }

    fn handle_f1_quote(
        &mut self,
        order_id: u64,
        quote: BboQuote,
        config: F1Config,
        cause: &KernelEvent,
        logical_ts_ns: i64,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
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
            events,
            ExecutionReconciliation {
                order_id: id,
                side,
                fills,
                status: outcome.status,
                uncertainty: map_f1_uncertainty(&outcome.uncertainty),
            },
        )
    }

    fn handle_f1_trade(
        &mut self,
        request: F1TradeRequest,
        cause: &KernelEvent,
        logical_ts_ns: i64,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
        self.require_tier(ExecutionTier::F1)?;
        self.observe_market_seq(request.trade.event_seq)?;
        let id = self.resolve_order_id(request.order_id)?;
        let order = self
            .orders
            .get(id)
            .ok_or_else(|| FacadeError::new(FacadeErrorCode::UnknownOrder))?;
        if order.time_in_force != TimeInForce::Gtc || !matches!(order.kind, OrderKind::Limit { .. })
        {
            return Err(FacadeError::new(
                FacadeErrorCode::UnsupportedOrderCombination,
            ));
        }
        let side = order.side;
        let outcome = execute_resting_on_trade(
            &mut self.orders,
            id,
            request.trade,
            request.trade.event_seq,
            logical_ts_ns,
            request.eligible_after_event_seq,
            request.displayed_ahead,
            &DisplayedAheadQueue,
            request.config,
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
            events,
            ExecutionReconciliation {
                order_id: id,
                side,
                fills,
                status: outcome.status,
                uncertainty: map_f1_uncertainty(&outcome.uncertainty),
            },
        )
    }

    fn handle_f2_snapshot(
        &mut self,
        snapshot: L2Snapshot,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
        self.require_tier(ExecutionTier::F2)?;
        self.observe_market_seq(snapshot.sequence)?;
        let sequence = snapshot.sequence;
        self.f2_book_mut()?
            .apply_snapshot(snapshot)
            .map_err(FacadeError::from_f2)?;
        emit(
            events,
            cause,
            DomainEventPayload::DepthSnapshotApplied { sequence },
        )
    }

    fn handle_f2_delta(
        &mut self,
        delta: L2Delta,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
        self.require_tier(ExecutionTier::F2)?;
        self.observe_market_seq(delta.sequence)?;
        self.f2_book_mut()?
            .apply_delta(delta)
            .map_err(FacadeError::from_f2)?;
        emit(
            events,
            cause,
            DomainEventPayload::DepthDeltaApplied {
                sequence: delta.sequence,
            },
        )
    }

    fn handle_f2_execution(
        &mut self,
        order_id: u64,
        config: SweepConfig,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
        self.require_tier(ExecutionTier::F2)?;
        let id = self.resolve_order_id(order_id)?;
        let side = self.order_side(id)?;
        let mut book = self
            .f2_book
            .take()
            .ok_or_else(|| FacadeError::new(FacadeErrorCode::InvalidSnapshot))?;
        let outcome =
            execute_taker(&mut self.orders, &mut book, id, config).map_err(FacadeError::from_f2)?;
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
            events,
            ExecutionReconciliation {
                order_id: id,
                side,
                fills,
                status: outcome.status,
                uncertainty: Vec::new(),
            },
        )
    }

    fn handle_leverage(
        &mut self,
        leverage: u8,
        equity: MoneyMinor,
        mark_price: PriceAtoms,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
        let requested = Leverage::new(leverage).map_err(FacadeError::from_risk)?;
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
            events,
            cause,
            DomainEventPayload::LeverageChanged {
                leverage: requested.get(),
                margin,
            },
        )
    }

    fn handle_risk(
        &mut self,
        equity: MoneyMinor,
        mark_price: PriceAtoms,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
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
            events,
            cause,
            DomainEventPayload::RiskEvaluated {
                margin,
                liquidation_state: self.risk.liquidation_state(),
                triggered,
            },
        )
    }

    fn handle_funding(
        &mut self,
        input: FundingInput,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
    ) -> Result<(), FacadeError> {
        let cash_delta = input.cash_delta;
        let posted = self
            .economics
            .post_funding(
                &mut self.ledger,
                ScheduledCashFlow {
                    id: input.id,
                    event_seq: cause.event_seq,
                    cash_delta,
                },
            )
            .map_err(FacadeError::from_economics)?;
        emit(
            events,
            cause,
            DomainEventPayload::FundingProcessed { posted, cash_delta },
        )
    }

    fn reconcile_execution(
        &mut self,
        cause: &KernelEvent,
        events: &mut Vec<DomainEvent>,
        result: ExecutionReconciliation,
    ) -> Result<(), FacadeError> {
        let ExecutionReconciliation {
            order_id,
            side,
            fills,
            status,
            uncertainty,
        } = result;
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
        )
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

    fn f2_book_mut(&mut self) -> Result<&mut crate::execution::f2::L2Book, FacadeError> {
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
