use sim_core::economics::{EconomicsMath, LiquidityRole};
use sim_core::execution::f2::{
    BookSide, DepthLevel, F2Error, L2Delta, L2Snapshot, SweepConfig,
};
use sim_core::facade::{
    DomainEvent, DomainEventPayload, ExecutionTier, FACADE_API_VERSION, FacadeConfig,
    FacadeErrorCode, FacadeInitialState, FacadeInput, FacadeRules, SimulatorFacade,
    SubmitOrderInput,
};
use sim_core::ledger::LedgerSnapshot;
use sim_core::numeric::{DecimalScale, PriceAtoms, QtyAtoms, RatePpb, Rounding};
use sim_core::orders::{NewOrder, OrderKind, Side, TimeInForce};
use sim_core::positions::{Position, PositionMath};
use sim_core::risk::{Leverage, RiskProfile};

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

fn facade() -> SimulatorFacade {
    let math = position_math();
    SimulatorFacade::new(
        FacadeConfig {
            api_version: FACADE_API_VERSION,
            session_id: "session-1".into(),
            instrument_id: "SYNTH".into(),
            execution_tier: ExecutionTier::F2,
            rules: FacadeRules {
                position_math: math,
                risk_profile: RiskProfile {
                    maintenance_rate: RatePpb::new(250_000_000),
                    math: EconomicsMath {
                        contract_multiplier_atoms: math.contract_multiplier_atoms,
                        qty_scale: math.qty_scale,
                        price_scale: math.price_scale,
                        multiplier_scale: math.multiplier_scale,
                        settlement_scale: math.settlement_scale,
                        rounding: math.rounding,
                    },
                },
                f0_fee_role: LiquidityRole::Taker,
                maker_fee_rate: RatePpb::new(0),
                taker_fee_rate: RatePpb::new(0),
            },
        },
        FacadeInitialState {
            position: Position::flat(),
            ledger: LedgerSnapshot {
                next_transaction_id: 0,
                balances: Vec::new(),
            },
            leverage: Leverage::new(2).unwrap(),
        },
    )
    .unwrap()
}

fn apply(facade: &mut SimulatorFacade, input: &FacadeInput, logical: i64) -> Vec<DomainEvent> {
    let sequence = facade.state_version();
    facade
        .apply(&input.envelope("session-1", sequence, sequence, logical))
        .unwrap()
}

fn snapshot(sequence: u64) -> FacadeInput {
    FacadeInput::F2Snapshot(L2Snapshot {
        sequence,
        bids: vec![DepthLevel {
            price: PriceAtoms::new(99),
            quantity: QtyAtoms::new(5),
        }],
        asks: vec![DepthLevel {
            price: PriceAtoms::new(101),
            quantity: QtyAtoms::new(5),
        }],
    })
}

#[test]
fn sequence_gap_is_consumed_quarantined_and_restorable() {
    let mut simulator = facade();
    apply(&mut simulator, &snapshot(5), 1);

    let events = apply(
        &mut simulator,
        &FacadeInput::F2Delta(L2Delta {
            previous_sequence: 3,
            sequence: 6,
            side: BookSide::Bid,
            price: PriceAtoms::new(98),
            quantity: QtyAtoms::new(2),
        }),
        2,
    );

    assert!(matches!(
        &events[0].payload,
        DomainEventPayload::DepthInvalidated {
            sequence: 6,
            reason: F2Error::SequenceGap,
        }
    ));
    assert_eq!(simulator.state_version(), 2);
    assert!(!simulator.f2_book().unwrap().is_enabled());
    assert_eq!(simulator.snapshot().market_event_seq, Some(6));

    let mut restored = SimulatorFacade::from_snapshot(simulator.snapshot()).unwrap();
    assert!(!restored.f2_book().unwrap().is_enabled());

    let submit = FacadeInput::SubmitOrder(SubmitOrderInput {
        request: NewOrder {
            client_order_id: "after-gap".into(),
            instrument_id: "SYNTH".into(),
            side: Side::Buy,
            quantity: QtyAtoms::new(1),
            kind: OrderKind::Market,
            time_in_force: TimeInForce::Gtc,
            reduce_only: false,
            post_only: false,
            marketable_only: false,
            submitted_at_event_seq: 6,
        },
        quote: None,
    });
    let submitted = apply(&mut restored, &submit, 3);
    let order_id = match &submitted[0].payload {
        DomainEventPayload::OrderSubmitted(order) => order.id.get(),
        _ => panic!("expected order submission"),
    };
    let before_execution = restored.snapshot();
    let execute = FacadeInput::ExecuteF2 {
        order_id,
        config: SweepConfig::default(),
    };
    let sequence = restored.state_version();
    let error = restored
        .apply(&execute.envelope("session-1", sequence, sequence, 4))
        .unwrap_err();
    assert_eq!(error.code, FacadeErrorCode::F2Execution);
    assert_eq!(restored.snapshot(), before_execution);

    apply(&mut restored, &snapshot(7), 5);
    assert!(restored.f2_book().unwrap().is_enabled());
    let execution = apply(&mut restored, &execute, 6);
    assert!(matches!(
        execution[0].payload,
        DomainEventPayload::Execution { .. }
    ));
}
