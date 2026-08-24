use super::*;
use crate::economics::{EconomicsMath, LiquidityRole, ScheduledEconomicId};
use crate::execution::f0::{Bar, F0Config};
use crate::execution::f1::{F1Config, TradePrint};
use crate::execution::f2::{DepthLevel, L2Book, L2Snapshot, SweepConfig};
use crate::hash::ZERO_HASH;
use crate::ledger::{LedgerAccount, LedgerSnapshot};
use crate::numeric::{DecimalScale, MoneyMinor, PriceAtoms, QtyAtoms, RatePpb, Rounding};
use crate::orders::{NewOrder, OrderId, OrderKind, Side, TimeInForce};
use crate::positions::{Position, PositionMath};
use crate::risk::{Leverage, RiskProfile};

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

fn apply(facade: &mut SimulatorFacade, input: &FacadeInput, logical: i64) -> Vec<DomainEvent> {
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
        &FacadeInput::SubmitOrder(SubmitOrderInput {
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
    assert_eq!(
        FacadeInput::decode(input.kind(), &payload),
        Ok(input.clone())
    );
    let mut trailing = payload;
    trailing.push(0);
    assert_eq!(
        FacadeInput::decode(input.kind(), &trailing),
        Err(FacadeError::new(FacadeErrorCode::InvalidPayload))
    );
}

#[test]
fn funding_has_one_canonical_form_without_caller_event_sequence() {
    let input = FacadeInput::Funding(FundingInput {
        id: ScheduledEconomicId::new("provider", "funding-1").unwrap(),
        cash_delta: MoneyMinor::new(-7),
    });
    let payload = input.canonical_payload();
    assert_eq!(FacadeInput::decode(input.kind(), &payload), Ok(input));
}

#[test]
fn malformed_embedded_quote_fails_during_decode() {
    let input = FacadeInput::SubmitOrder(SubmitOrderInput {
        request: market_order(0),
        quote: Some(crate::orders::TopOfBook {
            bid: PriceAtoms::new(110),
            ask: PriceAtoms::new(100),
        }),
    });
    assert_eq!(
        FacadeInput::decode(input.kind(), &input.canonical_payload()),
        Err(FacadeError::new(FacadeErrorCode::InvalidPayload))
    );
}

#[test]
fn unsupported_tier_fails_with_stable_code() {
    let error = SimulatorFacade::new(config(ExecutionTier::F3), initial()).unwrap_err();
    assert_eq!(error.code, FacadeErrorCode::UnsupportedExecutionTier);
    assert_eq!(
        ExecutionModelRegistry.supported(),
        [ExecutionTier::F0, ExecutionTier::F1, ExecutionTier::F2]
    );
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
        &FacadeInput::ExecuteF0 {
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
    assert!(matches!(
        events[0].payload,
        DomainEventPayload::Execution { .. }
    ));
    assert!(
        events
            .iter()
            .any(|event| matches!(event.payload, DomainEventPayload::FeePosted { .. }))
    );
    assert_eq!(facade.position().quantity_atoms, 2);
    assert_eq!(
        facade.position().average_entry_price,
        Some(PriceAtoms::new(100))
    );
    assert_eq!(
        facade.ledger().balance(LedgerAccount::Fees),
        MoneyMinor::new(2)
    );
    assert_eq!(
        facade.ledger().balance(LedgerAccount::Cash),
        MoneyMinor::new(-2)
    );
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
    let expected = apply(&mut uninterrupted, &execution, 2);

    let mut restored = SimulatorFacade::from_snapshot(snapshot).unwrap();
    let actual = apply(&mut restored, &execution, 2);
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
        &FacadeInput::F2Snapshot(L2Snapshot {
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
    let expected = apply(&mut facade, &execution, 3);
    let mut restored = SimulatorFacade::from_snapshot(snapshot).unwrap();
    let actual = apply(&mut restored, &execution, 3);
    assert_eq!(actual, expected);
    assert_eq!(restored.snapshot(), facade.snapshot());
}

#[test]
fn inconsistent_f2_snapshot_frontier_is_rejected() {
    let facade = SimulatorFacade::new(config(ExecutionTier::F2), initial()).unwrap();
    let mut snapshot = facade.snapshot();
    let mut book = L2Book::new();
    book.apply_snapshot(L2Snapshot {
        sequence: 9,
        bids: vec![DepthLevel {
            price: PriceAtoms::new(99),
            quantity: QtyAtoms::new(1),
        }],
        asks: vec![DepthLevel {
            price: PriceAtoms::new(101),
            quantity: QtyAtoms::new(1),
        }],
    })
    .unwrap();
    snapshot.f2_book = Some(book);
    assert_eq!(
        SimulatorFacade::from_snapshot(snapshot),
        Err(FacadeError::new(FacadeErrorCode::InvalidSnapshot))
    );
}

#[test]
fn funding_idempotency_survives_snapshot_restore() {
    let mut facade = SimulatorFacade::new(config(ExecutionTier::F0), initial()).unwrap();
    let funding = FacadeInput::Funding(FundingInput {
        id: ScheduledEconomicId::new("provider", "funding-1").unwrap(),
        cash_delta: MoneyMinor::new(-7),
    });
    let first = apply(&mut facade, &funding, 1);
    assert!(matches!(
        first[0].payload,
        DomainEventPayload::FundingProcessed { posted: true, .. }
    ));
    let snapshot = facade.snapshot();
    let mut restored = SimulatorFacade::from_snapshot(snapshot).unwrap();
    let second = apply(&mut restored, &funding, 2);
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
fn risk_paths_remain_exact_and_versioned() {
    let mut facade = SimulatorFacade::new(config(ExecutionTier::F0), initial()).unwrap();
    let leverage = apply(
        &mut facade,
        &FacadeInput::SetLeverage {
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
        &FacadeInput::EvaluateRisk {
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
