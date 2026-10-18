package practice

import (
	"errors"
	"fmt"
	"math/big"
)

// Fixed-point units used by the package.
const (
	StartingEquityCents int64 = 1_000_000
	HalfSpreadCents     int64 = 500
	TakerFeePPB         int64 = 500_000
	MakerFeePPB         int64 = 200_000
	MaintenanceRatePPB  int64 = 5_000_000
	MaxOrderMilli       int64 = 100_000
	MaxSafeJSONInteger  int64 = 9_007_199_254_740_991
)

type Intent string

const (
	IntentBuy     Intent = "BUY"
	IntentSell    Intent = "SELL"
	IntentShort   Intent = "SHORT"
	IntentCover   Intent = "COVER"
	IntentReverse Intent = "REVERSE"
	IntentClose   Intent = "CLOSE"
)

type Side string

const (
	SideBuy  Side = "BUY"
	SideSell Side = "SELL"
)

type PriceMode string

const (
	PriceMarket PriceMode = "MARKET"
	PriceBid    PriceMode = "BID"
	PriceAsk    PriceMode = "ASK"
	PriceMid    PriceMode = "MID"
	PriceStop   PriceMode = "STOP"
)

type OrderStatus string

const (
	OrderFilled   OrderStatus = "FILLED"
	OrderWorking  OrderStatus = "WORKING"
	OrderCanceled OrderStatus = "CANCELED"
	OrderRejected OrderStatus = "REJECTED"
)

type SessionStatus string

const (
	SessionActive     SessionStatus = "ACTIVE"
	SessionCompleted  SessionStatus = "COMPLETED"
	SessionLiquidated SessionStatus = "LIQUIDATED"
)

// Error is a stable simulator rejection suitable for translation by an API.
type Error struct {
	Code       string `json:"code"`
	HTTPStatus int    `json:"-"`
	Message    string `json:"message"`
}

func (e *Error) Error() string { return e.Message }

// OrderRequest is the complete player input accepted by SubmitOrder. Bid, ask,
// and midpoint are visible-price shortcuts resolved by the simulator.
type OrderRequest struct {
	Intent         Intent    `json:"intent"`
	QuantityMilli  int64     `json:"quantityMilli"`
	PriceMode      PriceMode `json:"priceMode"`
	StopPriceCents int64     `json:"stopPriceCents,omitempty"`
	ReduceOnly     bool      `json:"reduceOnly"`
	MakerOnly      bool      `json:"postOnly"`
	TakerOnly      bool      `json:"marketableOnly"`
}

// Order records both requested behavior and its terminal or resting outcome.
type Order struct {
	ID                int64       `json:"id"`
	Intent            Intent      `json:"intent"`
	Side              Side        `json:"side"`
	QuantityMilli     int64       `json:"quantityMilli"`
	FilledMilli       int64       `json:"filledMilli"`
	RemainingMilli    int64       `json:"remainingMilli"`
	PriceCents        *int64      `json:"priceCents"`
	StopPriceCents    *int64      `json:"stopPriceCents"`
	ReduceOnly        bool        `json:"reduceOnly"`
	MakerOnly         bool        `json:"postOnly"`
	TakerOnly         bool        `json:"marketableOnly"`
	Status            OrderStatus `json:"status"`
	ReasonCode        string      `json:"reasonCode"`
	Message           string      `json:"message"`
	CreatedAtStep     int         `json:"createdAtStep"`
	SimulatedMaker    bool        `json:"simulatedMaker"`
	SystemLiquidation bool        `json:"systemLiquidation"`
}

type RevealedPrice struct {
	Step       int   `json:"step"`
	PriceCents int64 `json:"priceCents"`
}

// Snapshot is the only player-safe view of a Session. Revealed never includes
// an entry after Step.
type Snapshot struct {
	SessionID              string          `json:"sessionId"`
	Instrument             string          `json:"instrument"`
	Fidelity               string          `json:"fidelity"`
	Status                 SessionStatus   `json:"status"`
	Step                   int             `json:"step"`
	TotalSteps             int             `json:"totalSteps"`
	Revealed               []RevealedPrice `json:"revealed"`
	MarkPriceCents         int64           `json:"markPriceCents"`
	BidPriceCents          int64           `json:"bidPriceCents"`
	AskPriceCents          int64           `json:"askPriceCents"`
	PositionMilli          int64           `json:"positionMilli"`
	AverageEntryCents      *int64          `json:"averageEntryCents"`
	RealizedPnLCents       int64           `json:"realizedPnlCents"`
	UnrealizedPnLCents     int64           `json:"unrealizedPnlCents"`
	FeesCents              int64           `json:"feesCents"`
	EquityCents            int64           `json:"equityCents"`
	StartingEquityCents    int64           `json:"startingEquityCents"`
	Leverage               int             `json:"leverage"`
	EffectiveLeverageMilli int64           `json:"effectiveLeverageMilli"`
	InitialMarginCents     int64           `json:"initialMarginCents"`
	MaintenanceMarginCents int64           `json:"maintenanceMarginCents"`
	AvailableMarginCents   int64           `json:"availableMarginCents"`
	LiquidationBufferCents int64           `json:"liquidationBufferCents"`
	MaxDrawdownCents       int64           `json:"maxDrawdownCents"`
	PeakEquityCents        int64           `json:"peakEquityCents"`
	Orders                 []Order         `json:"orders"`
}

// Session owns all authoritative simulation state. Its fields are unexported so
// serializing a Session cannot accidentally expose the sealed future path.
type Session struct {
	id             string
	pricesCents    []int64
	cursor         int
	status         SessionStatus
	startingEquity int64
	leverage       int
	positionMilli  int64
	averageEntry   *int64
	realizedPnL    int64
	fees           int64
	peakEquity     int64
	maxDrawdown    int64
	nextOrderID    int64
	orders         []Order
}

var practicePricesCents = [...]int64{
	6_425_000, 6_431_500, 6_418_000, 6_446_000, 6_459_500, 6_451_000,
	6_473_000, 6_498_000, 6_482_500, 6_511_000, 6_527_500, 6_504_000,
	6_476_000, 6_442_500, 6_455_500, 6_421_000, 6_388_000, 6_405_500,
	6_371_000, 6_346_500, 6_329_000, 6_358_500, 6_392_000, 6_417_500,
	6_446_500, 6_428_000, 6_461_000, 6_489_500, 6_517_000, 6_548_000,
	6_532_500, 6_571_000, 6_603_000, 6_584_500, 6_621_000, 6_647_500,
	6_619_000, 6_592_000, 6_613_500, 6_654_000, 6_683_500, 6_661_000,
	6_702_000, 6_736_500, 6_714_000, 6_752_500, 6_781_000, 6_765_500,
}

// NewSession creates a deterministic practice run. The caller supplies the
// opaque session identifier so the core never depends on randomness or time.
func NewSession(id string) *Session {
	prices := make([]int64, len(practicePricesCents))
	copy(prices, practicePricesCents[:])
	return &Session{
		id:             id,
		pricesCents:    prices,
		status:         SessionActive,
		startingEquity: StartingEquityCents,
		leverage:       5,
		peakEquity:     StartingEquityCents,
		nextOrderID:    1,
	}
}

// Snapshot returns a defensive, player-safe copy of current state.
func (s *Session) Snapshot() Snapshot {
	mark := s.currentPrice()
	unrealized := s.unrealizedPnL(mark)
	equity := s.startingEquity + s.realizedPnL + unrealized - s.fees
	notional := mulDiv(abs64(s.positionMilli), mark, 1_000)
	initialMargin := mulDiv(notional, 1, int64(s.leverage))
	maintenance := mulDiv(notional, MaintenanceRatePPB, 1_000_000_000)
	effectiveLeverage := int64(0)
	if notional > 0 && equity > 0 {
		effectiveLeverage = mulDiv(notional, 1_000, equity)
	} else if notional > 0 {
		effectiveLeverage = 50_000
	}

	revealed := make([]RevealedPrice, s.cursor+1)
	for i := range revealed {
		revealed[i] = RevealedPrice{Step: i, PriceCents: s.pricesCents[i]}
	}
	orders := make([]Order, len(s.orders))
	for i := range s.orders {
		orders[len(s.orders)-1-i] = cloneOrder(s.orders[i])
	}

	return Snapshot{
		SessionID:              s.id,
		Instrument:             "SYNTH-BTC-USD",
		Fidelity:               "F1 SIMULATED",
		Status:                 s.status,
		Step:                   s.cursor,
		TotalSteps:             len(s.pricesCents) - 1,
		Revealed:               revealed,
		MarkPriceCents:         mark,
		BidPriceCents:          mark - HalfSpreadCents,
		AskPriceCents:          mark + HalfSpreadCents,
		PositionMilli:          s.positionMilli,
		AverageEntryCents:      cloneInt64(s.averageEntry),
		RealizedPnLCents:       s.realizedPnL,
		UnrealizedPnLCents:     unrealized,
		FeesCents:              s.fees,
		EquityCents:            equity,
		StartingEquityCents:    s.startingEquity,
		Leverage:               s.leverage,
		EffectiveLeverageMilli: effectiveLeverage,
		InitialMarginCents:     initialMargin,
		MaintenanceMarginCents: maintenance,
		AvailableMarginCents:   equity - initialMargin,
		LiquidationBufferCents: equity - maintenance,
		MaxDrawdownCents:       s.maxDrawdown,
		PeakEquityCents:        s.peakEquity,
		Orders:                 orders,
	}
}

func (s *Session) SetLeverage(leverage int) (Snapshot, error) {
	if err := s.requireActive(); err != nil {
		return Snapshot{}, err
	}
	if leverage < 1 || leverage > 50 {
		return Snapshot{}, invalid("INVALID_LEVERAGE", "leverage must be an integer from 1x through 50x")
	}
	before := s.leverage
	s.leverage = leverage
	state := s.Snapshot()
	if state.AvailableMarginCents < 0 {
		s.leverage = before
		return Snapshot{}, conflict("INSUFFICIENT_AVAILABLE_MARGIN", "lower leverage would require more margin than is available")
	}
	return state, nil
}

func (s *Session) SubmitOrder(request OrderRequest) (Snapshot, error) {
	if err := s.requireActive(); err != nil {
		return Snapshot{}, err
	}
	if err := validateOrder(request); err != nil {
		return Snapshot{}, err
	}
	resolved := s.resolveIntent(request.Intent, request.QuantityMilli)
	order := Order{
		ID:             s.nextOrderID,
		Intent:         request.Intent,
		Side:           resolved.side,
		QuantityMilli:  resolved.quantity,
		RemainingMilli: resolved.quantity,
		PriceCents:     s.priceForMode(resolved.side, request.PriceMode),
		ReduceOnly:     request.ReduceOnly || resolved.forceReduceOnly,
		MakerOnly:      request.MakerOnly,
		TakerOnly:      request.TakerOnly || request.PriceMode == PriceMarket,
		Status:         OrderWorking,
		ReasonCode:     "ACCEPTED",
		Message:        "accepted",
		CreatedAtStep:  s.cursor,
	}
	if request.PriceMode == PriceStop {
		order.StopPriceCents = int64Ptr(request.StopPriceCents)
		order.Message = "waiting for stop trigger"
	}
	s.nextOrderID++
	s.orders = append(s.orders, order)
	orderIndex := len(s.orders) - 1

	if order.QuantityMilli == 0 {
		s.rejectOrder(orderIndex, "NO_OPEN_POSITION", "there is no open position for this action")
		return s.Snapshot(), nil
	}
	if request.PriceMode == PriceStop {
		return s.Snapshot(), nil
	}

	marketable := s.isMarketable(s.orders[orderIndex])
	if order.MakerOnly && marketable {
		s.rejectOrder(orderIndex, "POST_ONLY_WOULD_TAKE", "maker-only order would take liquidity at the visible quote")
		return s.Snapshot(), nil
	}
	if order.TakerOnly && !marketable {
		s.cancelOrder(orderIndex, "MARKETABLE_ONLY_UNFILLED", "taker-only order was not marketable and never rested")
		return s.Snapshot(), nil
	}
	if request.PriceMode == PriceMarket || marketable {
		s.fillOrder(orderIndex, s.executionPrice(order.Side), true)
	} else if order.MakerOnly {
		s.orders[orderIndex].Message = "maker-only order is resting at the visible quote"
	} else {
		s.orders[orderIndex].Message = "limit order is resting"
	}
	s.updateRisk()
	return s.Snapshot(), nil
}

// Advance applies one through eight future source events, revealing each event
// before any order processing based on it.
func (s *Session) Advance(steps int) (Snapshot, error) {
	if err := s.requireActive(); err != nil {
		return Snapshot{}, err
	}
	if steps < 1 || steps > 8 {
		return Snapshot{}, invalid("INVALID_ADVANCE", "advance steps must be an integer from 1 through 8")
	}
	for count := 0; count < steps && s.status == SessionActive; count++ {
		if s.cursor >= len(s.pricesCents)-1 {
			s.cancelWorkingOrders("SESSION_COMPLETED", "working order canceled when the practice run completed")
			s.status = SessionCompleted
			break
		}
		s.cursor++
		s.processWorkingOrders()
		s.updateRisk()
		if s.cursor >= len(s.pricesCents)-1 && s.status == SessionActive {
			s.cancelWorkingOrders("SESSION_COMPLETED", "working order canceled when the practice run completed")
			s.status = SessionCompleted
		}
	}
	return s.Snapshot(), nil
}

type resolvedIntent struct {
	side            Side
	quantity        int64
	forceReduceOnly bool
}

func (s *Session) resolveIntent(intent Intent, requested int64) resolvedIntent {
	switch intent {
	case IntentBuy:
		return resolvedIntent{side: SideBuy, quantity: requested}
	case IntentSell, IntentShort:
		return resolvedIntent{side: SideSell, quantity: requested}
	case IntentCover:
		return resolvedIntent{side: SideBuy, quantity: requested, forceReduceOnly: true}
	case IntentClose:
		side := SideSell
		if s.positionMilli < 0 {
			side = SideBuy
		}
		return resolvedIntent{side: side, quantity: abs64(s.positionMilli), forceReduceOnly: true}
	case IntentReverse:
		side := SideSell
		if s.positionMilli < 0 {
			side = SideBuy
		}
		return resolvedIntent{side: side, quantity: abs64(s.positionMilli) + requested}
	default:
		panic("validated intent became invalid")
	}
}

func validateOrder(request OrderRequest) error {
	switch request.Intent {
	case IntentBuy, IntentSell, IntentShort, IntentCover, IntentReverse, IntentClose:
	default:
		return invalid("INVALID_INTENT", "unknown order intent")
	}
	switch request.PriceMode {
	case PriceMarket, PriceBid, PriceAsk, PriceMid, PriceStop:
	default:
		return invalid("INVALID_PRICE_MODE", "unknown price mode")
	}
	if request.QuantityMilli <= 0 || request.QuantityMilli > MaxOrderMilli {
		return invalid("INVALID_QUANTITY", "quantity must be from 1 through 100000 milli-units")
	}
	if request.MakerOnly && request.TakerOnly {
		return invalid("INCOMPATIBLE_FLAGS", "maker-only and taker-only cannot both be enabled")
	}
	if request.PriceMode == PriceMarket && request.MakerOnly {
		return invalid("INCOMPATIBLE_FLAGS", "a market order cannot be maker-only")
	}
	if request.PriceMode == PriceStop && request.MakerOnly {
		return invalid("INCOMPATIBLE_FLAGS", "a stop-market order cannot be maker-only")
	}
	if request.PriceMode == PriceStop && (request.StopPriceCents <= 0 || request.StopPriceCents > MaxSafeJSONInteger) {
		return invalid("INVALID_STOP_PRICE", "a stop order requires a positive JavaScript-safe integer price")
	}
	return nil
}

func (s *Session) priceForMode(side Side, mode PriceMode) *int64 {
	mark := s.currentPrice()
	switch mode {
	case PriceBid:
		return int64Ptr(mark - HalfSpreadCents)
	case PriceAsk:
		return int64Ptr(mark + HalfSpreadCents)
	case PriceMid:
		return int64Ptr(mark)
	case PriceMarket, PriceStop:
		return nil
	default:
		panic("validated price mode became invalid")
	}
}

func (s *Session) isMarketable(order Order) bool {
	if order.PriceCents == nil {
		return true
	}
	if order.Side == SideBuy {
		return *order.PriceCents >= s.currentPrice()+HalfSpreadCents
	}
	return *order.PriceCents <= s.currentPrice()-HalfSpreadCents
}

func (s *Session) executionPrice(side Side) int64 {
	if side == SideBuy {
		return s.currentPrice() + HalfSpreadCents
	}
	return s.currentPrice() - HalfSpreadCents
}

func (s *Session) processWorkingOrders() {
	mark := s.currentPrice()
	for i := range s.orders {
		order := s.orders[i]
		if order.Status != OrderWorking {
			continue
		}
		if order.StopPriceCents != nil {
			triggered := order.Side == SideSell && mark <= *order.StopPriceCents ||
				order.Side == SideBuy && mark >= *order.StopPriceCents
			if triggered {
				s.fillOrder(i, s.executionPrice(order.Side), true)
			}
			continue
		}
		crossed := order.PriceCents != nil && (order.Side == SideBuy && mark <= *order.PriceCents ||
			order.Side == SideSell && mark >= *order.PriceCents)
		if crossed {
			s.fillOrder(i, *order.PriceCents, false)
		}
	}
}

func (s *Session) fillOrder(index int, fillPriceCents int64, taker bool) {
	order := &s.orders[index]
	fillQuantity := order.RemainingMilli
	if order.ReduceOnly {
		reducible := max64(0, s.positionMilli)
		if order.Side == SideBuy {
			reducible = max64(0, -s.positionMilli)
		}
		fillQuantity = min64(fillQuantity, reducible)
	}
	if fillQuantity == 0 {
		s.cancelOrder(index, "REDUCE_ONLY_EXHAUSTED", "reduce-only capacity was exhausted; no exposure was opened")
		return
	}

	signedFill := fillQuantity
	if order.Side == SideSell {
		signedFill = -signedFill
	}
	projected := s.projectPosition(signedFill, fillPriceCents)
	feeRate := MakerFeePPB
	if taker {
		feeRate = TakerFeePPB
	}
	fee := mulDiv(mulDiv(fillQuantity, fillPriceCents, 1_000), feeRate, 1_000_000_000)
	projectedUnrealized := int64(0)
	if projected.positionMilli != 0 && projected.averageEntry != nil {
		projectedUnrealized = mulDiv(projected.positionMilli, s.currentPrice()-*projected.averageEntry, 1_000)
	}
	projectedEquity := s.startingEquity + projected.realizedPnL + projectedUnrealized - s.fees - fee
	projectedNotional := mulDiv(abs64(projected.positionMilli), s.currentPrice(), 1_000)
	requiresMargin := createsOrIncreasesExposure(s.positionMilli, projected.positionMilli)
	if !order.SystemLiquidation && requiresMargin && mulDiv(projectedNotional, 1, int64(s.leverage)) > projectedEquity {
		s.rejectOrder(index, "INSUFFICIENT_MARGIN", "insufficient available margin for this fill")
		return
	}

	s.positionMilli = projected.positionMilli
	s.averageEntry = cloneInt64(projected.averageEntry)
	s.realizedPnL = projected.realizedPnL
	s.fees += fee
	order.FilledMilli += fillQuantity
	order.RemainingMilli -= fillQuantity
	order.PriceCents = int64Ptr(fillPriceCents)
	order.SimulatedMaker = !taker
	if order.RemainingMilli > 0 {
		order.Status = OrderCanceled
		order.ReasonCode = "REDUCE_ONLY_EXHAUSTED"
		order.Message = "reduce-only fill completed the closable quantity; excess was canceled"
		return
	}
	order.Status = OrderFilled
	order.ReasonCode = "FILLED"
	if taker {
		order.Message = "taker fill"
	} else {
		order.Message = "simulated maker fill"
	}
}

type projectedPosition struct {
	positionMilli int64
	averageEntry  *int64
	realizedPnL   int64
}

func (s *Session) projectPosition(signedFillMilli, fillPriceCents int64) projectedPosition {
	before := s.positionMilli
	after := before + signedFillMilli
	sameDirection := before == 0 || sign(before) == sign(signedFillMilli)
	if sameDirection {
		total := abs64(after)
		priorEntry := fillPriceCents
		if s.averageEntry != nil {
			priorEntry = *s.averageEntry
		}
		weighted := abs64(before)*priorEntry + abs64(signedFillMilli)*fillPriceCents
		var average *int64
		if total != 0 {
			average = int64Ptr(weighted / total)
		}
		return projectedPosition{positionMilli: after, averageEntry: average, realizedPnL: s.realizedPnL}
	}

	closing := min64(abs64(before), abs64(signedFillMilli))
	direction := int64(1)
	if before < 0 {
		direction = -1
	}
	entry := fillPriceCents
	if s.averageEntry != nil {
		entry = *s.averageEntry
	}
	realized := s.realizedPnL + mulDiv(closing, direction*(fillPriceCents-entry), 1_000)
	var average *int64
	if after != 0 {
		if sign(after) == sign(before) {
			average = cloneInt64(s.averageEntry)
		} else {
			average = int64Ptr(fillPriceCents)
		}
	}
	return projectedPosition{positionMilli: after, averageEntry: average, realizedPnL: realized}
}

func (s *Session) updateRisk() {
	state := s.Snapshot()
	if state.EquityCents > s.peakEquity {
		s.peakEquity = state.EquityCents
	}
	drawdown := s.peakEquity - state.EquityCents
	if drawdown > s.maxDrawdown {
		s.maxDrawdown = drawdown
	}
	if state.EquityCents > state.MaintenanceMarginCents || s.positionMilli == 0 {
		return
	}
	s.cancelWorkingOrders("LIQUIDATION_CANCEL", "working order canceled before liquidation")
	liquidation := Order{
		ID:                s.nextOrderID,
		Intent:            IntentClose,
		Side:              SideSell,
		QuantityMilli:     abs64(s.positionMilli),
		RemainingMilli:    abs64(s.positionMilli),
		ReduceOnly:        true,
		TakerOnly:         true,
		Status:            OrderWorking,
		ReasonCode:        "SYSTEM_LIQUIDATION",
		Message:           "system liquidation",
		CreatedAtStep:     s.cursor,
		SystemLiquidation: true,
	}
	if s.positionMilli < 0 {
		liquidation.Side = SideBuy
	}
	s.nextOrderID++
	s.orders = append(s.orders, liquidation)
	s.fillOrder(len(s.orders)-1, s.executionPrice(liquidation.Side), true)
	s.status = SessionLiquidated
}

func (s *Session) currentPrice() int64 { return s.pricesCents[s.cursor] }

func (s *Session) unrealizedPnL(mark int64) int64 {
	if s.positionMilli == 0 || s.averageEntry == nil {
		return 0
	}
	return mulDiv(s.positionMilli, mark-*s.averageEntry, 1_000)
}

func (s *Session) requireActive() error {
	if s.status != SessionActive {
		return conflict("SESSION_ENDED", "this run has ended; start a new fixture to trade again")
	}
	return nil
}

func (s *Session) rejectOrder(index int, code, message string) {
	s.orders[index].Status = OrderRejected
	s.orders[index].ReasonCode = code
	s.orders[index].Message = message
}

func (s *Session) cancelOrder(index int, code, message string) {
	s.orders[index].Status = OrderCanceled
	s.orders[index].ReasonCode = code
	s.orders[index].Message = message
}

func (s *Session) cancelWorkingOrders(code, message string) {
	for i := range s.orders {
		if s.orders[i].Status == OrderWorking {
			s.cancelOrder(i, code, message)
		}
	}
}

func invalid(code, message string) error {
	return &Error{Code: code, HTTPStatus: 400, Message: message}
}

func conflict(code, message string) error {
	return &Error{Code: code, HTTPStatus: 409, Message: message}
}

func cloneOrder(order Order) Order {
	order.PriceCents = cloneInt64(order.PriceCents)
	order.StopPriceCents = cloneInt64(order.StopPriceCents)
	return order
}

func cloneInt64(value *int64) *int64 {
	if value == nil {
		return nil
	}
	copy := *value
	return &copy
}

func int64Ptr(value int64) *int64 { return &value }

func abs64(value int64) int64 {
	if value < 0 {
		return -value
	}
	return value
}

func min64(a, b int64) int64 {
	if a < b {
		return a
	}
	return b
}

func max64(a, b int64) int64 {
	if a > b {
		return a
	}
	return b
}

func sign(value int64) int {
	switch {
	case value < 0:
		return -1
	case value > 0:
		return 1
	default:
		return 0
	}
}

// createsOrIncreasesExposure distinguishes opening risk from reducing an
// existing position. A zero-crossing fill always needs a margin check for its
// residual opposite-side exposure, while a close or same-side reduction must
// remain possible even when the account is already below initial margin.
func createsOrIncreasesExposure(before, after int64) bool {
	if after == 0 {
		return false
	}
	if before == 0 || sign(before) != sign(after) {
		return true
	}
	return abs64(after) > abs64(before)
}

// mulDiv performs exact integer multiplication followed by truncation toward
// zero. big.Int avoids silent overflow while preserving fixed-point semantics.
func mulDiv(left, right, divisor int64) int64 {
	if divisor == 0 {
		panic("practice: zero fixed-point divisor")
	}
	product := new(big.Int).Mul(big.NewInt(left), big.NewInt(right))
	quotient := new(big.Int).Quo(product, big.NewInt(divisor))
	if !quotient.IsInt64() {
		panic(fmt.Sprintf("practice: fixed-point overflow: (%d * %d) / %d", left, right, divisor))
	}
	return quotient.Int64()
}

// IsErrorCode permits API layers to branch without parsing human messages.
func IsErrorCode(err error, code string) bool {
	var practiceErr *Error
	return errors.As(err, &practiceErr) && practiceErr.Code == code
}
