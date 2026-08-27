#![allow(clippy::too_many_lines)]

use sim_core::economics::{
    EconomicsMath, EconomicsState, ExecutionFeeInput, LiquidityRole, ScheduledCashFlow,
    ScheduledEconomicId, SplitRatio, split_order, split_position,
};
use sim_core::execution::f0::{Bar, F0Config, IntrabarPolicy, UncertaintyFlag, execute_bar};
use sim_core::execution::f1::{
    BboQuote, F1Config, F1Error, F1LiquidityRole, MidpointRounding, QuoteReference, TradePrint,
    execute_on_quote, execute_resting_on_trade, quote_reference_price,
};
use sim_core::execution::f2::{
    BookSide, DepthLevel, F2Error, L2Book, L2Delta, L2Snapshot, QueuePosition, SweepConfig,
    execute_resting_trade, execute_taker,
};
use sim_core::facade::{
    DomainEvent, DomainEventPayload, ExecutionTier, FACADE_API_VERSION, FacadeConfig,
    FacadeInitialState, FacadeInput, FacadeRules, SimulatorFacade, SubmitOrderInput,
};
use sim_core::hash::{ZERO_HASH, hash_hex, sha256};
use sim_core::instrument::{AssetClass, InstrumentDefinition, ProductType, SettlementKind};
use sim_core::kernel::{InputEnvelope, Kernel, KernelError};
use sim_core::ledger::{
    Ledger, LedgerAccount, LedgerError, LedgerSnapshot, NewTransaction, Posting,
};
use sim_core::numeric::{
    DecimalScale, MoneyMinor, NumericError, PriceAtoms, QtyAtoms, RatePpb, Rounding, rescale_i64,
};
use sim_core::orders::{
    NewOrder, OrderError, OrderKind, OrderState, OrderStatus, Side, TimeInForce, TopOfBook,
};
use sim_core::positions::{FillSide, Position, PositionMath};
use sim_core::risk::{
    Leverage, LiquidationState, RiskError, RiskProfile, RiskState, margin_snapshot, precheck_fill,
};

const SESSION_ID: &str = "m1-13-conformance";
const EXPECTED_VECTOR_SET_HASH: &str =
    "029dd3eddec07972f44fcef00a498077038445f481087bc13a9d0e522c19d1d9";
const GOLDEN: &str = include_str!("../../../fixtures/scenarios/m1_13_expected.txt");

fn zero_scale() -> DecimalScale {
    DecimalScale::new(0).unwrap()
}

fn position_math() -> PositionMath {
    PositionMath {
        contract_multiplier_atoms: 1,
        qty_scale: zero_scale(),
        price_scale: zero_scale(),
        multiplier_scale: zero_scale(),
        settlement_scale: zero_scale(),
        rounding: Rounding::TowardZero,
    }
}

fn economics_math() -> EconomicsMath {
    let math = position_math();
    EconomicsMath {
        contract_multiplier_atoms: math.contract_multiplier_atoms,
        qty_scale: math.qty_scale,
        price_scale: math.price_scale,
        multiplier_scale: math.multiplier_scale,
        settlement_scale: math.settlement_scale,
        rounding: math.rounding,
    }
}

fn risk_profile() -> RiskProfile {
    RiskProfile {
        maintenance_rate: RatePpb::new(250_000_000),
        math: economics_math(),
    }
}

fn position(quantity_atoms: i64, basis: i64, realized: i64) -> Position {
    Position::from_snapshot(Position {
        quantity_atoms,
        average_entry_price: (quantity_atoms != 0).then_some(PriceAtoms::new(basis)),
        realized_pnl: MoneyMinor::new(realized),
    })
    .unwrap()
}

fn order(side: Side, quantity: u64, kind: OrderKind) -> NewOrder {
    NewOrder {
        client_order_id: "conformance-order".into(),
        instrument_id: "SYNTH".into(),
        side,
        quantity: QtyAtoms::new(quantity),
        kind,
        time_in_force: TimeInForce::Gtc,
        reduce_only: false,
        post_only: false,
        marketable_only: false,
        submitted_at_event_seq: 1,
    }
}

fn bar(event_seq: u64, open: i64, high: i64, low: i64, close: i64) -> Bar {
    Bar {
        event_seq,
        open: PriceAtoms::new(open),
        high: PriceAtoms::new(high),
        low: PriceAtoms::new(low),
        close: PriceAtoms::new(close),
        base_volume: QtyAtoms::new(1_000),
    }
}

fn depth_level(price: i64, quantity: u64) -> DepthLevel {
    DepthLevel {
        price: PriceAtoms::new(price),
        quantity: QtyAtoms::new(quantity),
    }
}

fn depth_snapshot(sequence: u64) -> L2Snapshot {
    L2Snapshot {
        sequence,
        bids: vec![depth_level(99, 5), depth_level(100, 4)],
        asks: vec![
            depth_level(101, 3),
            depth_level(102, 4),
            depth_level(103, 5),
        ],
    }
}

fn run_scenario(name: &str) -> String {
    match name {
        "long_partial_close" => long_partial_close(),
        "sell_through_zero" => sell_through_zero(),
        "buy_through_zero" => buy_through_zero(),
        "reduce_only_excess" => reduce_only_excess(),
        "reduce_only_race" => reduce_only_race(),
        "reverse_margin_partial" => reverse_margin_partial(),
        "post_only_marketable" => post_only_marketable(),
        "marketable_only_partial" => marketable_only_partial(),
        "midpoint_rounding" => midpoint_rounding(),
        "stop_gap" => stop_gap(),
        "stop_limit_miss" => stop_limit_miss(),
        "ambiguous_bar" => ambiguous_bar(),
        "l2_depth_sweep" => l2_depth_sweep(),
        "maker_queue" => maker_queue(),
        "funding_liquidation" => funding_liquidation(),
        "lower_leverage_reject" => lower_leverage_reject(),
        "increase_leverage" => increase_leverage(),
        "fee_rebate" => fee_rebate(),
        "futures_expiry" => futures_expiry(),
        "split_working_order" => split_working_order(),
        "sequence_gap" => sequence_gap(),
        "duplicate_command" => duplicate_command(),
        "snapshot_resume" => snapshot_resume(),
        "integer_extremes" => integer_extremes(),
        other => panic!("unknown M1-13 scenario: {other}"),
    }
}

fn long_partial_close() -> String {
    let mut state = Position::flat();
    state
        .apply_fill(
            FillSide::Buy,
            QtyAtoms::new(6),
            PriceAtoms::new(100),
            position_math(),
        )
        .unwrap();
    state
        .apply_fill(
            FillSide::Sell,
            QtyAtoms::new(2),
            PriceAtoms::new(110),
            position_math(),
        )
        .unwrap();
    format!(
        "qty={},basis={},realized={}",
        state.quantity_atoms,
        state.average_entry_price.unwrap().get(),
        state.realized_pnl.get()
    )
}

fn sell_through_zero() -> String {
    let mut state = position(5, 100, 0);
    let legs = state
        .apply_fill(
            FillSide::Sell,
            QtyAtoms::new(8),
            PriceAtoms::new(110),
            position_math(),
        )
        .unwrap();
    format!(
        "qty={},basis={},realized={},legs={}",
        state.quantity_atoms,
        state.average_entry_price.unwrap().get(),
        state.realized_pnl.get(),
        legs.len()
    )
}

fn buy_through_zero() -> String {
    let mut state = position(-5, 100, 0);
    let legs = state
        .apply_fill(
            FillSide::Buy,
            QtyAtoms::new(8),
            PriceAtoms::new(90),
            position_math(),
        )
        .unwrap();
    format!(
        "qty={},basis={},realized={},legs={}",
        state.quantity_atoms,
        state.average_entry_price.unwrap().get(),
        state.realized_pnl.get(),
        legs.len()
    )
}

fn reduce_only_excess() -> String {
    let mut orders = OrderState::new();
    let mut request = order(Side::Sell, 8, OrderKind::Market);
    request.reduce_only = true;
    let outcome = orders.submit(request, 5, None).unwrap();
    orders
        .record_fill(outcome.order_id, outcome.accepted_quantity)
        .unwrap();
    let mut state = position(5, 100, 0);
    state
        .apply_fill(
            FillSide::Sell,
            outcome.accepted_quantity,
            PriceAtoms::new(100),
            position_math(),
        )
        .unwrap();
    let accepted = outcome.accepted_quantity.get();
    let current_order = orders.get(outcome.order_id).unwrap();
    format!(
        "requested=8,accepted={accepted},rejected_excess={},filled={},final_qty={},status={:?}",
        8 - accepted,
        current_order.filled.get(),
        state.quantity_atoms,
        current_order.status
    )
}

fn reduce_only_race() -> String {
    let mut orders = OrderState::new();
    let mut first = order(Side::Sell, 3, OrderKind::Market);
    first.reduce_only = true;
    first.client_order_id = "first".into();
    let first_outcome = orders.submit(first, 5, None).unwrap();

    let mut second = order(Side::Sell, 5, OrderKind::Market);
    second.reduce_only = true;
    second.client_order_id = "second".into();
    let second_outcome = orders.submit(second, 5, None).unwrap();

    let mut state = position(5, 100, 0);
    orders
        .record_fill(first_outcome.order_id, first_outcome.accepted_quantity)
        .unwrap();
    state
        .apply_fill(
            FillSide::Sell,
            first_outcome.accepted_quantity,
            PriceAtoms::new(100),
            position_math(),
        )
        .unwrap();
    orders
        .record_fill(second_outcome.order_id, second_outcome.accepted_quantity)
        .unwrap();
    state
        .apply_fill(
            FillSide::Sell,
            second_outcome.accepted_quantity,
            PriceAtoms::new(100),
            position_math(),
        )
        .unwrap();

    format!(
        "first_accepted={},second_accepted={},final_qty={},first_status={:?},second_status={:?}",
        first_outcome.accepted_quantity.get(),
        second_outcome.accepted_quantity.get(),
        state.quantity_atoms,
        orders.get(first_outcome.order_id).unwrap().status,
        orders.get(second_outcome.order_id).unwrap().status
    )
}

fn reverse_margin_partial() -> String {
    let decision = precheck_fill(
        MoneyMinor::new(75),
        position(5, 100, 17),
        Side::Sell,
        QtyAtoms::new(8),
        PriceAtoms::new(100),
        MoneyMinor::new(0),
        Leverage::new(2).unwrap(),
        risk_profile(),
    )
    .unwrap();
    format!(
        "close={},requested_open={},accepted_open={},accepted_total={},full={}",
        decision.close_quantity.get(),
        decision.requested_open_quantity.get(),
        decision.accepted_open_quantity.get(),
        decision.accepted_quantity.get(),
        decision.fully_accepted()
    )
}

fn post_only_marketable() -> String {
    let quote = TopOfBook::new(PriceAtoms::new(99), PriceAtoms::new(100)).unwrap();
    let mut buy = order(
        Side::Buy,
        1,
        OrderKind::Limit {
            limit_price: PriceAtoms::new(100),
        },
    );
    buy.post_only = true;
    let buy_error = OrderState::new().submit(buy, 0, Some(quote)).unwrap_err();

    let mut sell = order(
        Side::Sell,
        1,
        OrderKind::Limit {
            limit_price: PriceAtoms::new(99),
        },
    );
    sell.post_only = true;
    let sell_error = OrderState::new().submit(sell, 0, Some(quote)).unwrap_err();
    assert_eq!(buy_error, OrderError::PostOnlyWouldTake);
    assert_eq!(sell_error, OrderError::PostOnlyWouldTake);
    "buy=PostOnlyWouldTake,sell=PostOnlyWouldTake".into()
}

fn marketable_only_partial() -> String {
    let top = TopOfBook::new(PriceAtoms::new(99), PriceAtoms::new(100)).unwrap();
    let mut orders = OrderState::new();
    let mut request = order(
        Side::Buy,
        5,
        OrderKind::Limit {
            limit_price: PriceAtoms::new(102),
        },
    );
    request.time_in_force = TimeInForce::Ioc;
    request.marketable_only = true;
    let id = orders.submit(request, 0, Some(top)).unwrap().order_id;
    let outcome = execute_on_quote(
        &mut orders,
        id,
        BboQuote {
            event_seq: 2,
            event_time_ns: 10,
            bid: PriceAtoms::new(99),
            bid_size: QtyAtoms::new(4),
            ask: PriceAtoms::new(100),
            ask_size: QtyAtoms::new(3),
        },
        2,
        10,
        F1Config::default(),
    )
    .unwrap();
    assert_eq!(outcome.liquidity_role, Some(F1LiquidityRole::Taker));
    format!(
        "filled={},price={},status={:?},role=Taker",
        outcome.filled.get(),
        outcome.fill_price.unwrap().get(),
        outcome.status
    )
}

fn midpoint_rounding() -> String {
    let quote = BboQuote {
        event_seq: 1,
        event_time_ns: 1,
        bid: PriceAtoms::new(100),
        bid_size: QtyAtoms::new(1),
        ask: PriceAtoms::new(103),
        ask_size: QtyAtoms::new(1),
    };
    let buy = quote_reference_price(quote, QuoteReference::Midpoint(MidpointRounding::TowardBid))
        .unwrap();
    let sell = quote_reference_price(quote, QuoteReference::Midpoint(MidpointRounding::TowardAsk))
        .unwrap();
    format!(
        "buy_passive_mid={},sell_passive_mid={}",
        buy.get(),
        sell.get()
    )
}

fn stop_gap() -> String {
    let mut orders = OrderState::new();
    let id = orders
        .submit(
            order(
                Side::Buy,
                10,
                OrderKind::StopMarket {
                    stop_price: PriceAtoms::new(105),
                },
            ),
            0,
            None,
        )
        .unwrap()
        .order_id;
    let outcome = execute_bar(
        &mut orders,
        id,
        bar(2, 110, 115, 108, 112),
        F0Config {
            intrabar_policy: IntrabarPolicy::Pessimistic,
            market_slippage_atoms: 3,
        },
    )
    .unwrap();
    format!(
        "triggered={},filled={},price={},status={:?}",
        outcome.triggered,
        outcome.filled.get(),
        outcome.fill_price.unwrap().get(),
        outcome.status
    )
}

fn stop_limit_miss() -> String {
    let mut orders = OrderState::new();
    let id = orders
        .submit(
            order(
                Side::Buy,
                10,
                OrderKind::StopLimit {
                    stop_price: PriceAtoms::new(105),
                    limit_price: PriceAtoms::new(106),
                },
            ),
            0,
            None,
        )
        .unwrap()
        .order_id;
    let outcome = execute_bar(
        &mut orders,
        id,
        bar(2, 110, 115, 108, 112),
        F0Config::default(),
    )
    .unwrap();
    format!(
        "triggered={},filled={},status={:?},uncertainty={}",
        outcome.triggered,
        outcome.filled.get(),
        outcome.status,
        outcome.uncertainty.len()
    )
}

fn ambiguous_bar() -> String {
    let mut orders = OrderState::new();
    let id = orders
        .submit(
            order(
                Side::Buy,
                10,
                OrderKind::StopLimit {
                    stop_price: PriceAtoms::new(105),
                    limit_price: PriceAtoms::new(100),
                },
            ),
            0,
            None,
        )
        .unwrap()
        .order_id;
    let outcome = execute_bar(
        &mut orders,
        id,
        bar(2, 102, 110, 95, 104),
        F0Config::default(),
    )
    .unwrap();
    assert_eq!(
        outcome.uncertainty,
        vec![UncertaintyFlag::IntrabarAmbiguous]
    );
    format!(
        "triggered={},filled={},status={:?},warning=IntrabarAmbiguous",
        outcome.triggered,
        outcome.filled.get(),
        outcome.status
    )
}

fn l2_depth_sweep() -> String {
    let mut book = L2Book::new();
    book.apply_snapshot(depth_snapshot(2)).unwrap();
    let mut orders = OrderState::new();
    let id = orders
        .submit(order(Side::Buy, 6, OrderKind::Market), 0, None)
        .unwrap()
        .order_id;
    let outcome = execute_taker(&mut orders, &mut book, id, SweepConfig::default()).unwrap();
    format!(
        "filled={},levels={}:{},{}:{},status={:?},ask102_remaining={}",
        outcome.filled.get(),
        outcome.levels[0].price.get(),
        outcome.levels[0].quantity.get(),
        outcome.levels[1].price.get(),
        outcome.levels[1].quantity.get(),
        outcome.status,
        book.quantity_at(BookSide::Ask, PriceAtoms::new(102)).get()
    )
}

fn maker_queue() -> String {
    let mut orders = OrderState::new();
    let mut request = order(
        Side::Buy,
        5,
        OrderKind::Limit {
            limit_price: PriceAtoms::new(100),
        },
    );
    request.submitted_at_event_seq = 10;
    let id = orders.submit(request, 0, None).unwrap().order_id;
    let mut queue = QueuePosition::join(QtyAtoms::new(3));
    let filled = execute_resting_trade(
        &mut orders,
        &mut queue,
        id,
        11,
        PriceAtoms::new(100),
        QtyAtoms::new(5),
    )
    .unwrap();
    let current = orders.get(id).unwrap();
    format!(
        "filled={},queue_ahead={},order_filled={},status={:?}",
        filled.get(),
        queue.ahead().get(),
        current.filled.get(),
        current.status
    )
}

fn funding_liquidation() -> String {
    let mut ledger = Ledger::from_snapshot(LedgerSnapshot {
        next_transaction_id: 0,
        balances: vec![
            (LedgerAccount::Cash, MoneyMinor::new(101)),
            (LedgerAccount::PositionCost, MoneyMinor::new(-101)),
        ],
    })
    .unwrap();
    let state_position = position(4, 100, 0);
    let mut risk = RiskState::new(Leverage::new(2).unwrap());
    let mut economics = EconomicsState::new();
    let before = margin_snapshot(
        ledger.balance(LedgerAccount::Cash),
        state_position,
        "SYNTH",
        &OrderState::new(),
        PriceAtoms::new(100),
        risk.leverage(),
        risk_profile(),
    )
    .unwrap();
    assert_eq!(before.maintenance_margin, MoneyMinor::new(100));
    assert!(!risk.evaluate_liquidation(state_position, before));

    economics
        .post_funding(
            &mut ledger,
            ScheduledCashFlow {
                id: ScheduledEconomicId::new("fixture", "funding-1").unwrap(),
                event_seq: 7,
                cash_delta: MoneyMinor::new(-2),
            },
        )
        .unwrap();
    let after = margin_snapshot(
        ledger.balance(LedgerAccount::Cash),
        state_position,
        "SYNTH",
        &OrderState::new(),
        PriceAtoms::new(100),
        risk.leverage(),
        risk_profile(),
    )
    .unwrap();
    let triggered = risk.evaluate_liquidation(state_position, after);
    format!(
        "cash={},funding={},maintenance={},triggered={},state={:?}",
        ledger.balance(LedgerAccount::Cash).get(),
        ledger.balance(LedgerAccount::Funding).get(),
        after.maintenance_margin.get(),
        triggered,
        risk.liquidation_state()
    )
}

fn lower_leverage_reject() -> String {
    let mut risk = RiskState::new(Leverage::new(10).unwrap());
    let state_position = position(10, 100, 17);
    let result = risk.set_leverage(
        Leverage::new(1).unwrap(),
        MoneyMinor::new(500),
        state_position,
        "SYNTH",
        &OrderState::new(),
        PriceAtoms::new(100),
        risk_profile(),
    );
    assert_eq!(result, Err(RiskError::InsufficientMargin));
    format!(
        "result=InsufficientMargin,leverage={},qty={},realized={}",
        risk.leverage().get(),
        state_position.quantity_atoms,
        state_position.realized_pnl.get()
    )
}

fn increase_leverage() -> String {
    let mut risk = RiskState::new(Leverage::new(2).unwrap());
    let state_position = position(10, 100, 17);
    let margin = risk
        .set_leverage(
            Leverage::new(5).unwrap(),
            MoneyMinor::new(10_000),
            state_position,
            "SYNTH",
            &OrderState::new(),
            PriceAtoms::new(100),
            risk_profile(),
        )
        .unwrap();
    format!(
        "leverage={},initial_margin={},qty={},realized={}",
        risk.leverage().get(),
        margin.position_initial_margin.get(),
        state_position.quantity_atoms,
        state_position.realized_pnl.get()
    )
}

fn fee_rebate() -> String {
    let economics = EconomicsState::new();
    let mut ledger = Ledger::new();
    let fee = economics
        .post_execution_fee(
            &mut ledger,
            ExecutionFeeInput {
                event_seq: 1,
                quantity: QtyAtoms::new(2),
                price: PriceAtoms::new(100),
                rate: RatePpb::new(-10_000_000),
                role: LiquidityRole::Maker,
                math: economics_math(),
            },
        )
        .unwrap();
    let aggregate: i64 = ledger
        .snapshot()
        .balances
        .iter()
        .map(|(_, amount)| amount.get())
        .sum();
    format!(
        "fee={},cash={},fees={},balanced={}",
        fee.get(),
        ledger.balance(LedgerAccount::Cash).get(),
        ledger.balance(LedgerAccount::Fees).get(),
        aggregate == 0
    )
}

fn futures_expiry() -> String {
    let definition = InstrumentDefinition {
        instrument_id: "SYNTH-FUT".into(),
        venue_id: "SYNTH".into(),
        asset_class: AssetClass::Future,
        product_type: ProductType::Future,
        price_scale: zero_scale(),
        qty_scale: zero_scale(),
        tick_size_atoms: 1,
        qty_increment_atoms: 1,
        contract_multiplier_atoms: 1,
        multiplier_scale: zero_scale(),
        settlement_kind: SettlementKind::Linear,
        listing_ns: Some(0),
        expiry_ns: Some(100),
        effective_from_ns: 0,
        effective_through_ns: Some(200),
    };
    definition.validate().unwrap();
    let active_before = definition.is_active_at(99);
    let active_at_expiry = definition.is_active_at(100);

    let mut economics = EconomicsState::new();
    let mut ledger = Ledger::new();
    let flow = ScheduledCashFlow {
        id: ScheduledEconomicId::new("fixture", "expiry-1").unwrap(),
        event_seq: 9,
        cash_delta: MoneyMinor::new(12),
    };
    let posted = economics
        .post_settlement(&mut ledger, flow.clone())
        .unwrap();
    let retry = economics.post_settlement(&mut ledger, flow).unwrap();
    format!(
        "active_before={active_before},active_at_expiry={active_at_expiry},cash={},settlement={},posted={posted},retry={retry}",
        ledger.balance(LedgerAccount::Cash).get(),
        ledger.balance(LedgerAccount::Settlement).get()
    )
}

fn split_working_order() -> String {
    let adjusted_position =
        split_position(position(5, 100, 7), SplitRatio::new(2, 1).unwrap()).unwrap();
    let mut orders = OrderState::new();
    let id = orders
        .submit(
            order(
                Side::Sell,
                4,
                OrderKind::Limit {
                    limit_price: PriceAtoms::new(120),
                },
            ),
            5,
            None,
        )
        .unwrap()
        .order_id;
    let adjusted_order =
        split_order(orders.get(id).unwrap(), SplitRatio::new(2, 1).unwrap()).unwrap();
    let OrderKind::Limit { limit_price } = adjusted_order.kind else {
        panic!("split must preserve limit order kind");
    };
    format!(
        "position_qty={},position_basis={},realized={},order_qty={},order_filled={},limit={}",
        adjusted_position.quantity_atoms,
        adjusted_position.average_entry_price.unwrap().get(),
        adjusted_position.realized_pnl.get(),
        adjusted_order.quantity.get(),
        adjusted_order.filled.get(),
        limit_price.get()
    )
}

fn sequence_gap() -> String {
    let mut book = L2Book::new();
    book.apply_snapshot(depth_snapshot(20)).unwrap();
    let error = book
        .apply_delta(L2Delta {
            previous_sequence: 20,
            sequence: 22,
            side: BookSide::Ask,
            price: PriceAtoms::new(101),
            quantity: QtyAtoms::new(2),
        })
        .unwrap_err();
    assert_eq!(error, F2Error::SequenceGap);

    let mut orders = OrderState::new();
    let id = orders
        .submit(order(Side::Buy, 1, OrderKind::Market), 0, None)
        .unwrap()
        .order_id;
    let execution = execute_taker(&mut orders, &mut book, id, SweepConfig::default()).unwrap_err();
    assert_eq!(execution, F2Error::BookDisabled);
    format!(
        "error=SequenceGap,enabled={},retained_ask={},execution=BookDisabled",
        book.is_enabled(),
        book.quantity_at(BookSide::Ask, PriceAtoms::new(101)).get()
    )
}

fn duplicate_command() -> String {
    let input = InputEnvelope {
        session_id: SESSION_ID.into(),
        input_seq: 0,
        expected_state_version: 0,
        logical_ts_ns: 10,
        kind: "COMMAND".into(),
        payload: b"same".to_vec(),
    };
    let mut kernel = Kernel::new();
    kernel.apply(&input).unwrap();
    let after_first = kernel.snapshot();
    let same = kernel.apply(&input).unwrap_err();
    let mut changed = input;
    changed.payload = b"changed".to_vec();
    let changed_error = kernel.apply(&changed).unwrap_err();
    assert!(matches!(same, KernelError::InputSequence { .. }));
    assert!(matches!(changed_error, KernelError::InputSequence { .. }));
    let after_rejections = kernel.snapshot();
    format!(
        "same=InputSequence,changed=InputSequence,state_version={},state_unchanged={}",
        after_rejections.state_version,
        after_rejections == after_first
    )
}

fn facade_config() -> FacadeConfig {
    FacadeConfig {
        api_version: FACADE_API_VERSION,
        session_id: SESSION_ID.into(),
        instrument_id: "SYNTH".into(),
        execution_tier: ExecutionTier::F0,
        rules: FacadeRules {
            position_math: position_math(),
            risk_profile: risk_profile(),
            f0_fee_role: LiquidityRole::Taker,
            maker_fee_rate: RatePpb::new(0),
            taker_fee_rate: RatePpb::new(0),
        },
    }
}

fn apply_facade(
    facade: &mut SimulatorFacade,
    input: &FacadeInput,
    logical_ts_ns: i64,
) -> Vec<DomainEvent> {
    let sequence = facade.state_version();
    facade
        .apply(&input.envelope(SESSION_ID, sequence, sequence, logical_ts_ns))
        .unwrap()
}

fn snapshot_resume() -> String {
    let initial = FacadeInitialState {
        position: Position::flat(),
        ledger: LedgerSnapshot {
            next_transaction_id: 0,
            balances: Vec::new(),
        },
        leverage: Leverage::new(2).unwrap(),
    };
    let mut uninterrupted = SimulatorFacade::new(facade_config(), initial).unwrap();
    let submitted = apply_facade(
        &mut uninterrupted,
        &FacadeInput::SubmitOrder(SubmitOrderInput {
            request: NewOrder {
                client_order_id: "snapshot-order".into(),
                submitted_at_event_seq: 0,
                ..order(Side::Buy, 2, OrderKind::Market)
            },
            quote: None,
        }),
        1,
    );
    let order_id = match &submitted[0].payload {
        DomainEventPayload::OrderSubmitted(current_order) => current_order.id,
        _ => panic!("expected submitted order"),
    };
    let checkpoint = uninterrupted.snapshot();
    let execution = FacadeInput::ExecuteF0 {
        order_id: order_id.get(),
        bar: bar(1, 100, 100, 100, 100),
        config: F0Config::default(),
    };
    let expected_events = apply_facade(&mut uninterrupted, &execution, 2);
    let mut restored = SimulatorFacade::from_snapshot(checkpoint).unwrap();
    let actual_events = apply_facade(&mut restored, &execution, 2);
    let expected_snapshot = uninterrupted.snapshot();
    let actual_snapshot = restored.snapshot();
    let hash_equal =
        expected_snapshot.kernel.current_event_hash == actual_snapshot.kernel.current_event_hash;
    format!(
        "events_equal={},snapshots_equal={},hash_equal={hash_equal},hash_nonzero={}",
        expected_events == actual_events,
        expected_snapshot == actual_snapshot,
        actual_snapshot.kernel.current_event_hash != ZERO_HASH
    )
}

fn integer_extremes() -> String {
    let price_max = PriceAtoms::new(i64::MAX - 1)
        .checked_add(PriceAtoms::new(1))
        .unwrap();
    let price_overflow = PriceAtoms::new(i64::MAX).checked_add(PriceAtoms::new(1));
    let qty_overflow = QtyAtoms::new(u64::MAX).checked_add(QtyAtoms::new(1));
    let rescale = rescale_i64(
        i64::MAX,
        zero_scale(),
        DecimalScale::new(18).unwrap(),
        Rounding::TowardZero,
    );
    assert_eq!(price_overflow, Err(NumericError::Overflow));
    assert_eq!(qty_overflow, Err(NumericError::Overflow));
    assert_eq!(rescale, Err(NumericError::Overflow));
    format!(
        "price_max={},price_overflow=Overflow,qty_overflow=Overflow,rescale=Overflow",
        price_max.get()
    )
}

#[test]
fn required_golden_scenarios_match_committed_vectors() {
    let mut count = 0_usize;
    let mut canonical_vector_set = String::new();
    for line in GOLDEN
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let mut fields = line.splitn(3, '|');
        let name = fields.next().unwrap();
        let expected_hash = fields.next().unwrap();
        let expected_summary = fields.next().unwrap();
        let actual = run_scenario(name);
        assert_eq!(actual, expected_summary, "scenario {name} changed");
        assert_eq!(
            hash_hex(&sha256(actual.as_bytes())),
            expected_hash,
            "scenario {name} commitment changed"
        );
        canonical_vector_set.push_str(line);
        canonical_vector_set.push('\n');
        count += 1;
    }
    assert_eq!(
        count, 24,
        "all required docs/testing.md scenarios must be pinned"
    );
    assert_eq!(
        hash_hex(&sha256(canonical_vector_set.as_bytes())),
        EXPECTED_VECTOR_SET_HASH,
        "the ordered M1-13 vector set changed"
    );
}

fn next_seed(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state
}

#[test]
fn seeded_property_invariants_are_reproducible() {
    let mut seed = 0x4d31_3133_d37e_5eed_u64;
    let mut left_position = Position::flat();
    let mut right_position = Position::flat();
    let mut signed_fill_sum = 0_i64;
    let mut ledger = Ledger::new();

    for event_seq in 0_u64..256 {
        let value = next_seed(&mut seed);
        let quantity = QtyAtoms::new(value % 7 + 1);
        let price = PriceAtoms::new(i64::try_from(value % 101 + 50).unwrap());
        let side = if value & 1 == 0 {
            FillSide::Buy
        } else {
            FillSide::Sell
        };
        left_position
            .apply_fill(side, quantity, price, position_math())
            .unwrap();
        right_position
            .apply_fill(side, quantity, price, position_math())
            .unwrap();
        let signed = i64::try_from(quantity.get()).unwrap();
        signed_fill_sum += if side == FillSide::Buy {
            signed
        } else {
            -signed
        };

        let amount = i64::try_from((value >> 8) % 1_000 + 1).unwrap();
        ledger
            .record(NewTransaction {
                event_seq,
                kind: "PROPERTY_BALANCE".into(),
                postings: vec![
                    Posting {
                        account: LedgerAccount::Cash,
                        amount: MoneyMinor::new(amount),
                    },
                    Posting {
                        account: LedgerAccount::RealizedPnl,
                        amount: MoneyMinor::new(-amount),
                    },
                ],
            })
            .unwrap();
    }

    assert_eq!(left_position, right_position);
    assert_eq!(left_position.quantity_atoms, signed_fill_sum);
    let ledger_sum: i64 = ledger
        .snapshot()
        .balances
        .iter()
        .map(|(_, amount)| amount.get())
        .sum();
    assert_eq!(ledger_sum, 0);

    let inputs: Vec<_> = (0_u64..64)
        .map(|sequence| InputEnvelope {
            session_id: SESSION_ID.into(),
            input_seq: sequence,
            expected_state_version: sequence,
            logical_ts_ns: i64::try_from(sequence).unwrap(),
            kind: "PROPERTY".into(),
            payload: sequence.to_be_bytes().to_vec(),
        })
        .collect();
    let mut left_kernel = Kernel::new();
    let mut right_kernel = Kernel::new();
    let left_events = left_kernel.apply_batch(&inputs).unwrap();
    let right_events = right_kernel.apply_batch(&inputs).unwrap();
    assert_eq!(left_events, right_events);
    assert_eq!(left_kernel.snapshot(), right_kernel.snapshot());

    let mut prior = ZERO_HASH;
    for event in left_events {
        assert_eq!(event.prior_event_hash, prior);
        prior = event.current_event_hash;
    }
    assert_ne!(prior, ZERO_HASH);
}

#[test]
fn reduce_only_property_never_expands_or_flips_exposure() {
    for position_quantity in 1_i64..=20 {
        for requested in 1_u64..=25 {
            let mut orders = OrderState::new();
            let mut request = order(Side::Sell, requested, OrderKind::Market);
            request.reduce_only = true;
            let accepted = orders
                .submit(request, position_quantity, None)
                .unwrap()
                .accepted_quantity;
            let mut state = position(position_quantity, 100, 0);
            state
                .apply_fill(
                    FillSide::Sell,
                    accepted,
                    PriceAtoms::new(100),
                    position_math(),
                )
                .unwrap();
            assert!(state.quantity_atoms >= 0);
            assert!(state.quantity_atoms.unsigned_abs() <= position_quantity.unsigned_abs());
        }
    }
}

#[test]
fn critical_guard_mutation_targets_fail_closed() {
    let mut ledger = Ledger::new();
    let ledger_before = ledger.clone();
    assert_eq!(
        ledger.record(NewTransaction {
            event_seq: 1,
            kind: "UNBALANCED".into(),
            postings: vec![
                Posting {
                    account: LedgerAccount::Cash,
                    amount: MoneyMinor::new(5),
                },
                Posting {
                    account: LedgerAccount::Fees,
                    amount: MoneyMinor::new(-4),
                },
            ],
        }),
        Err(LedgerError::Unbalanced)
    );
    assert_eq!(ledger, ledger_before);

    let mut orders = OrderState::new();
    let order_id = orders
        .submit(order(Side::Buy, 2, OrderKind::Market), 0, None)
        .unwrap()
        .order_id;
    let order_before = orders.clone();
    assert_eq!(
        orders.record_fill(order_id, QtyAtoms::new(3)),
        Err(OrderError::Overfill)
    );
    assert_eq!(orders, order_before);

    let mut kernel = Kernel::new();
    let kernel_before = kernel.snapshot();
    let wrong_version = InputEnvelope {
        session_id: SESSION_ID.into(),
        input_seq: 0,
        expected_state_version: 1,
        logical_ts_ns: 1,
        kind: "GUARD".into(),
        payload: Vec::new(),
    };
    assert!(matches!(
        kernel.apply(&wrong_version),
        Err(KernelError::StateVersion { .. })
    ));
    assert_eq!(kernel.snapshot(), kernel_before);

    let mut resting_orders = OrderState::new();
    let resting_id = resting_orders
        .submit(
            order(
                Side::Buy,
                2,
                OrderKind::Limit {
                    limit_price: PriceAtoms::new(100),
                },
            ),
            0,
            None,
        )
        .unwrap()
        .order_id;
    let resting_before = resting_orders.clone();
    let future = execute_resting_on_trade(
        &mut resting_orders,
        resting_id,
        TradePrint {
            event_seq: 11,
            event_time_ns: 11,
            price: PriceAtoms::new(100),
            quantity: QtyAtoms::new(2),
        },
        10,
        11,
        1,
        QtyAtoms::new(0),
        &sim_core::execution::f1::DisplayedAheadQueue,
        F1Config::default(),
    );
    assert_eq!(future, Err(F1Error::FutureMarketData));
    assert_eq!(resting_orders, resting_before);

    let mut terminal = OrderState::new();
    let terminal_id = terminal
        .submit(order(Side::Buy, 1, OrderKind::Market), 0, None)
        .unwrap()
        .order_id;
    terminal.record_fill(terminal_id, QtyAtoms::new(1)).unwrap();
    assert_eq!(
        terminal.get(terminal_id).unwrap().status,
        OrderStatus::Filled
    );
    let terminal_before = terminal.clone();
    assert_eq!(
        terminal.record_fill(terminal_id, QtyAtoms::new(1)),
        Err(OrderError::InvalidState)
    );
    assert_eq!(terminal, terminal_before);

    assert_eq!(sequence_gap(), run_scenario("sequence_gap"));
    assert_eq!(LiquidationState::Triggered, {
        let mut risk = RiskState::new(Leverage::new(2).unwrap());
        let state_position = position(4, 100, 0);
        let snapshot = margin_snapshot(
            MoneyMinor::new(99),
            state_position,
            "SYNTH",
            &OrderState::new(),
            PriceAtoms::new(100),
            risk.leverage(),
            risk_profile(),
        )
        .unwrap();
        assert!(risk.evaluate_liquidation(state_position, snapshot));
        risk.liquidation_state()
    });
}
