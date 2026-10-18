package practice

import (
	"encoding/json"
	"reflect"
	"strconv"
	"strings"
	"testing"
)

func marketOrder(intent Intent, quantity int64) OrderRequest {
	return OrderRequest{
		Intent:        intent,
		QuantityMilli: quantity,
		PriceMode:     PriceMarket,
		TakerOnly:     true,
	}
}

func mustOrder(t *testing.T, session *Session, request OrderRequest) Snapshot {
	t.Helper()
	state, err := session.SubmitOrder(request)
	if err != nil {
		t.Fatalf("SubmitOrder() error = %v", err)
	}
	return state
}

func mustAdvance(t *testing.T, session *Session, steps int) Snapshot {
	t.Helper()
	state, err := session.Advance(steps)
	if err != nil {
		t.Fatalf("Advance() error = %v", err)
	}
	return state
}

func TestSnapshotNeverRevealsFuturePrices(t *testing.T) {
	session := NewSession("visibility-test")
	state := session.Snapshot()
	if len(state.Revealed) != 1 || state.Revealed[0].Step != 0 {
		t.Fatalf("new snapshot revealed %+v; want only step zero", state.Revealed)
	}
	if state.TotalSteps <= len(state.Revealed) {
		t.Fatalf("fixture needs a sealed future: total=%d revealed=%d", state.TotalSteps, len(state.Revealed))
	}

	encoded, err := json.Marshal(state)
	if err != nil {
		t.Fatalf("json.Marshal(snapshot): %v", err)
	}
	future := strconv.FormatInt(practicePricesCents[len(practicePricesCents)-1], 10)
	if strings.Contains(string(encoded), future) {
		t.Fatalf("snapshot JSON leaked future terminal price %s", future)
	}
	for _, point := range state.Revealed {
		if point.Step > state.Step {
			t.Fatalf("revealed step %d exceeded frontier %d", point.Step, state.Step)
		}
	}
}

func TestSessionsAreDeterministic(t *testing.T) {
	left := NewSession("same")
	right := NewSession("same")
	commands := []OrderRequest{
		marketOrder(IntentBuy, 100),
		{Intent: IntentSell, QuantityMilli: 250, PriceMode: PriceBid, MakerOnly: true},
		{Intent: IntentSell, QuantityMilli: 100, PriceMode: PriceStop, StopPriceCents: 6_420_000, ReduceOnly: true},
	}
	for _, command := range commands {
		mustOrder(t, left, command)
		mustOrder(t, right, command)
	}
	for i := 0; i < 4; i++ {
		mustAdvance(t, left, 3)
		mustAdvance(t, right, 3)
	}
	if !reflect.DeepEqual(left.Snapshot(), right.Snapshot()) {
		t.Fatal("identical sessions and commands produced different snapshots")
	}
}

func TestSellAcrossZeroRealizesCloseAndOpensShort(t *testing.T) {
	session := NewSession("reverse")
	mustOrder(t, session, marketOrder(IntentBuy, 100))
	state := mustOrder(t, session, marketOrder(IntentSell, 250))
	if state.PositionMilli != -150 {
		t.Fatalf("position = %d, want -150", state.PositionMilli)
	}
	if state.RealizedPnLCents >= 0 {
		t.Fatalf("realized P&L = %d, want spread loss", state.RealizedPnLCents)
	}
	if state.AverageEntryCents == nil || *state.AverageEntryCents != state.BidPriceCents {
		t.Fatalf("short basis = %v, want bid %d", state.AverageEntryCents, state.BidPriceCents)
	}
}

func TestReverseIntentCrossesToRequestedOppositeSize(t *testing.T) {
	session := NewSession("reverse-intent")
	mustOrder(t, session, marketOrder(IntentBuy, 100))
	state := mustOrder(t, session, marketOrder(IntentReverse, 80))
	if state.PositionMilli != -80 {
		t.Fatalf("position = %d, want -80", state.PositionMilli)
	}
	if state.Orders[0].QuantityMilli != 180 {
		t.Fatalf("reverse quantity = %d, want close 100 + open 80", state.Orders[0].QuantityMilli)
	}
}

func TestReduceOnlyFillsClosableQuantityAndCancelsExcess(t *testing.T) {
	session := NewSession("reduce-only")
	mustOrder(t, session, marketOrder(IntentBuy, 100))
	request := marketOrder(IntentSell, 250)
	request.ReduceOnly = true
	state := mustOrder(t, session, request)
	order := state.Orders[0]
	if state.PositionMilli != 0 {
		t.Fatalf("position = %d, reduce-only crossed zero", state.PositionMilli)
	}
	if order.FilledMilli != 100 || order.RemainingMilli != 150 || order.Status != OrderCanceled {
		t.Fatalf("order = %+v, want 100 filled and 150 canceled", order)
	}
	if order.ReasonCode != "REDUCE_ONLY_EXHAUSTED" {
		t.Fatalf("reason = %q, want REDUCE_ONLY_EXHAUSTED", order.ReasonCode)
	}
}

func TestCoverPreservesRequestedExcessForAuditableCancellation(t *testing.T) {
	session := NewSession("cover-excess")
	mustOrder(t, session, marketOrder(IntentShort, 100))
	state := mustOrder(t, session, marketOrder(IntentCover, 250))
	order := state.Orders[0]
	if state.PositionMilli != 0 {
		t.Fatalf("position = %d, cover crossed zero", state.PositionMilli)
	}
	if order.QuantityMilli != 250 || order.FilledMilli != 100 || order.RemainingMilli != 150 {
		t.Fatalf("cover = %+v, want requested 250, filled 100, remaining 150", order)
	}
	if order.Status != OrderCanceled || order.ReasonCode != "REDUCE_ONLY_EXHAUSTED" {
		t.Fatalf("cover outcome = %+v, want machine-readable excess cancellation", order)
	}
}

func TestMakerOnlyRejectsMarketableVisibleQuote(t *testing.T) {
	session := NewSession("maker")
	state := mustOrder(t, session, OrderRequest{
		Intent:        IntentBuy,
		QuantityMilli: 100,
		PriceMode:     PriceAsk,
		MakerOnly:     true,
	})
	order := state.Orders[0]
	if order.Status != OrderRejected || order.ReasonCode != "POST_ONLY_WOULD_TAKE" {
		t.Fatalf("order = %+v, want post-only rejection", order)
	}
}

func TestTakerOnlyNeverRests(t *testing.T) {
	session := NewSession("taker")
	state := mustOrder(t, session, OrderRequest{
		Intent:        IntentBuy,
		QuantityMilli: 100,
		PriceMode:     PriceBid,
		TakerOnly:     true,
	})
	order := state.Orders[0]
	if order.Status != OrderCanceled || order.ReasonCode != "MARKETABLE_ONLY_UNFILLED" {
		t.Fatalf("order = %+v, want non-resting cancellation", order)
	}
}

func TestStopUsesOnlyLaterRevealedPrint(t *testing.T) {
	session := NewSession("stop")
	mustOrder(t, session, marketOrder(IntentBuy, 100))
	state := mustOrder(t, session, OrderRequest{
		Intent:         IntentSell,
		QuantityMilli:  100,
		PriceMode:      PriceStop,
		StopPriceCents: 6_420_000,
		ReduceOnly:     true,
	})
	if state.Orders[0].Status != OrderWorking {
		t.Fatalf("stop status = %s, want working before later print", state.Orders[0].Status)
	}
	for state.Orders[0].Status == OrderWorking {
		state = mustAdvance(t, session, 1)
	}
	if state.Orders[0].Status != OrderFilled || state.PositionMilli != 0 {
		t.Fatalf("triggered state = %+v, want stop fill and flat position", state)
	}
	if state.Orders[0].CreatedAtStep >= state.Step {
		t.Fatalf("stop filled without a later event: created=%d current=%d", state.Orders[0].CreatedAtStep, state.Step)
	}
}

func TestStopPriceRejectsJSONUnsafeInteger(t *testing.T) {
	session := NewSession("unsafe-stop")
	_, err := session.SubmitOrder(OrderRequest{
		Intent:         IntentSell,
		QuantityMilli:  100,
		PriceMode:      PriceStop,
		StopPriceCents: MaxSafeJSONInteger + 1,
	})
	if !IsErrorCode(err, "INVALID_STOP_PRICE") {
		t.Fatalf("SubmitOrder() error = %v, want INVALID_STOP_PRICE", err)
	}
}

func TestLeverageBoundsAndDoesNotResizePosition(t *testing.T) {
	session := NewSession("leverage")
	before := mustOrder(t, session, marketOrder(IntentBuy, 100))
	after, err := session.SetLeverage(20)
	if err != nil {
		t.Fatalf("SetLeverage(20): %v", err)
	}
	if after.Leverage != 20 || after.PositionMilli != before.PositionMilli {
		t.Fatalf("after = %+v, want leverage 20 and unchanged position %d", after, before.PositionMilli)
	}
	if _, err := session.SetLeverage(51); !IsErrorCode(err, "INVALID_LEVERAGE") {
		t.Fatalf("SetLeverage(51) error = %v, want INVALID_LEVERAGE", err)
	}
}

func TestInvalidLeverageDecreaseIsAtomic(t *testing.T) {
	session := NewSession("atomic-leverage")
	if _, err := session.SetLeverage(50); err != nil {
		t.Fatalf("SetLeverage(50): %v", err)
	}
	mustOrder(t, session, marketOrder(IntentBuy, 7_000))
	before := session.Snapshot()
	if _, err := session.SetLeverage(1); !IsErrorCode(err, "INSUFFICIENT_AVAILABLE_MARGIN") {
		t.Fatalf("SetLeverage(1) error = %v, want INSUFFICIENT_AVAILABLE_MARGIN", err)
	}
	after := session.Snapshot()
	if after.Leverage != before.Leverage || after.PositionMilli != before.PositionMilli {
		t.Fatalf("failed leverage change mutated state: before=%+v after=%+v", before, after)
	}
}

func TestNegativeAvailableMarginDoesNotBlockRiskReduction(t *testing.T) {
	session := NewSession("under-margin-reduction")
	if _, err := session.SetLeverage(50); err != nil {
		t.Fatalf("SetLeverage(50): %v", err)
	}
	mustOrder(t, session, marketOrder(IntentBuy, 5_000))
	state := mustAdvance(t, session, 8)
	state = mustAdvance(t, session, 8)
	state = mustAdvance(t, session, 4)
	if state.Step != 20 {
		t.Fatalf("step = %d, want 20", state.Step)
	}
	if state.AvailableMarginCents >= 0 {
		t.Fatalf("available margin = %d, reproduction requires a negative value", state.AvailableMarginCents)
	}

	reduction := marketOrder(IntentSell, 1_000)
	reduction.ReduceOnly = true
	state = mustOrder(t, session, reduction)
	if state.PositionMilli != 4_000 {
		t.Fatalf("position = %d, want risk-reduced 4000", state.PositionMilli)
	}
	if state.Orders[0].Status != OrderFilled || state.Orders[0].FilledMilli != 1_000 {
		t.Fatalf("reduction order = %+v, want filled 1000", state.Orders[0])
	}
}

func TestNewExposureAdmissionIncludesVisibleSpreadLoss(t *testing.T) {
	session := NewSession("spread-aware-margin")
	if _, err := session.SetLeverage(50); err != nil {
		t.Fatalf("SetLeverage(50): %v", err)
	}
	state := mustOrder(t, session, marketOrder(IntentBuy, 7_590))
	order := state.Orders[0]
	if order.Status != OrderRejected || order.ReasonCode != "INSUFFICIENT_MARGIN" {
		t.Fatalf("near-boundary order = %+v, want spread-aware margin rejection", order)
	}
	if state.PositionMilli != 0 || state.AvailableMarginCents < 0 {
		t.Fatalf("rejected order mutated account: position=%d available=%d", state.PositionMilli, state.AvailableMarginCents)
	}
}

func TestUnderMarginReversalStillChecksOppositeResidual(t *testing.T) {
	session := NewSession("under-margin-reversal")
	if _, err := session.SetLeverage(50); err != nil {
		t.Fatalf("SetLeverage(50): %v", err)
	}
	mustOrder(t, session, marketOrder(IntentBuy, 5_000))
	mustAdvance(t, session, 8)
	mustAdvance(t, session, 8)
	before := mustAdvance(t, session, 4)
	if before.AvailableMarginCents >= 0 {
		t.Fatalf("available margin = %d, reproduction requires a negative value", before.AvailableMarginCents)
	}

	state := mustOrder(t, session, marketOrder(IntentSell, 13_000))
	order := state.Orders[0]
	if order.Status != OrderRejected || order.ReasonCode != "INSUFFICIENT_MARGIN" {
		t.Fatalf("reversal order = %+v, want residual exposure rejection", order)
	}
	if state.PositionMilli != before.PositionMilli {
		t.Fatalf("rejected reversal changed position from %d to %d", before.PositionMilli, state.PositionMilli)
	}
}

func TestAdvanceCompletesDeterministicRun(t *testing.T) {
	session := NewSession("complete")
	state := session.Snapshot()
	for state.Status == SessionActive {
		state = mustAdvance(t, session, 8)
	}
	if state.Status != SessionCompleted {
		t.Fatalf("status = %s, want completed", state.Status)
	}
	if len(state.Revealed) != state.TotalSteps+1 {
		t.Fatalf("revealed=%d total=%d", len(state.Revealed), state.TotalSteps)
	}
}

func TestCompletionCancelsEveryWorkingOrder(t *testing.T) {
	session := NewSession("completion-cancel")
	state := mustOrder(t, session, OrderRequest{
		Intent:         IntentBuy,
		QuantityMilli:  100,
		PriceMode:      PriceStop,
		StopPriceCents: 10_000_000,
	})
	if state.Orders[0].Status != OrderWorking {
		t.Fatalf("unreachable stop status = %s, want working", state.Orders[0].Status)
	}

	for state.Status == SessionActive {
		state = mustAdvance(t, session, 8)
	}
	if state.Status != SessionCompleted {
		t.Fatalf("status = %s, want completed", state.Status)
	}
	for _, order := range state.Orders {
		if order.Status == OrderWorking {
			t.Fatalf("completed session retained working order: %+v", order)
		}
	}
	order := state.Orders[0]
	if order.Status != OrderCanceled || order.ReasonCode != "SESSION_COMPLETED" {
		t.Fatalf("terminal order = %+v, want SESSION_COMPLETED cancellation", order)
	}
}

func TestLiquidationClosesExposureAndEndsRun(t *testing.T) {
	session := NewSession("liquidation")
	if _, err := session.SetLeverage(50); err != nil {
		t.Fatalf("SetLeverage(50): %v", err)
	}
	// The sealed path eventually rallies far enough to breach maintenance on
	// this near-maximum 50x short.
	mustOrder(t, session, marketOrder(IntentShort, 7_500))
	state := session.Snapshot()
	for state.Status == SessionActive {
		state = mustAdvance(t, session, 1)
	}
	if state.Status != SessionLiquidated {
		t.Fatalf("status = %s, want liquidated", state.Status)
	}
	if state.PositionMilli != 0 || len(state.Orders) == 0 || !state.Orders[0].SystemLiquidation {
		t.Fatalf("liquidation state = %+v, want flat with system order", state)
	}
}

func TestBankruptLiquidationStillClosesExposure(t *testing.T) {
	session := NewSession("bankruptcy")
	entry := session.currentPrice()
	session.startingEquity = 1
	session.positionMilli = -100
	session.averageEntry = &entry
	session.cursor = len(session.pricesCents) - 1

	session.updateRisk()
	state := session.Snapshot()
	if state.Status != SessionLiquidated || state.PositionMilli != 0 {
		t.Fatalf("bankrupt liquidation = %+v, want ended flat position", state)
	}
	if len(state.Orders) == 0 || state.Orders[0].Status != OrderFilled {
		t.Fatalf("liquidation order = %+v, want a completed system close", state.Orders)
	}
}
