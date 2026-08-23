# Trading, accounting, and risk rules

This document is normative. Examples use decimal notation for readability; implementations use fixed-point integers with instrument-defined scales.

## 1. Core state

For one v1 account/instrument:

- `position_qty`: signed quantity; positive is long, negative is short.
- `cash`: settled account currency after fills, fees, funding, and adjustments.
- `avg_entry_price`: entry basis for the currently open sign only.
- `mark_price`: ruleset-selected mark, never an unrevealed future value.
- `equity = cash + unrealized_pnl` under the instrument's settlement formula.
- `notional = abs(position_qty) * mark_price * contract_multiplier` for linear products.
- `effective_leverage = notional / equity` when equity is positive.
- `requested_leverage`: 1×–50× margin setting; this is a ceiling/requirement input, not a P&L multiplier.
- `initial_margin = notional / requested_leverage` in the v1 synthetic isolated profile.
- `maintenance_margin = notional * maintenance_rate + maintenance_fixed` using the active tier.

Inverse contracts and futures variation margin require instrument-specific calculators behind the same interface.

## 2. Canonical order command

UI verbs are presets. The engine receives:

```text
OrderCommand {
  command_id, session_id, instrument_id,
  side: BUY | SELL,
  quantity,
  order_type: MARKET | LIMIT | STOP_MARKET | STOP_LIMIT,
  limit_price?, stop_price?,
  time_in_force: GTC | IOC | FOK,
  reduce_only: bool,
  post_only: bool,
  marketable_only: bool,
  slippage_cap_bps?,
  submitted_at_event_seq,
  client_idempotency_key
}
```

Validation order is: schema → session state → instrument increments → incompatible flags → reference-price snapshot → reduce-only capacity → margin precheck → post/marketability check → acceptance.

`post_only` and `marketable_only` are mutually exclusive. Market orders imply marketable-only behavior and cannot be post-only. FOK requires a fidelity tier that can estimate immediately executable quantity; otherwise it is unavailable.

## 3. UI intent mapping

- **Buy / long:** `side=BUY`. If currently short, fills cover first and then open long.
- **Sell:** `side=SELL`. If currently long, fills close first and then open short.
- **Short:** `side=SELL`; the UI asks for either order quantity or target negative position.
- **Cover:** `side=BUY, reduce_only=true` by default.
- **Close:** side opposite current sign, quantity equal to absolute position, `reduce_only=true`.
- **Reverse by size:** order quantity `abs(current) + desired_opposite_size`.
- **Reverse same size:** order quantity `2 * abs(current)` on the opposite side.
- **Target position:** UI computes delta as `target_qty - current_qty`, then sends the side and absolute delta. The engine records both target intent and resolved command for audit.

## 4. Crossing zero and fill legs

Each fill is applied atomically but may generate two accounting legs:

Example: current `+5`, sell fill `8` at 100.

1. Close leg: sell `5`, realize P&L against the long average entry.
2. Open leg: sell `3`, establish a short with entry 100.
3. Charge fees on total filled quantity `8` according to each execution's liquidity role.

If the open leg fails margin precheck, the order may fill only the closable quantity when partial fills are permitted; the remainder cancels as `INSUFFICIENT_MARGIN_TO_REVERSE`. FOK rejects the entire order.

With `reduce_only=true`, executable quantity is capped at the live reducible position at every fill event, not merely at acceptance. It cannot increase exposure, cross zero, or reopen after another order closes the position. Excess cancels as `REDUCE_ONLY_EXHAUSTED`.

## 5. Price shortcuts

Bid, ask, and midpoint are ticket price selectors, not distinct order types.

- `bid`: current revealed best bid.
- `ask`: current revealed best ask.
- `midpoint`: rounded to the instrument tick using a declared policy; default is toward the passive side (buy rounds down, sell rounds up).
- If the required quote is stale or absent, the selector is disabled.
- The engine stores the observed quote event ID, raw computed value, rounded limit price, and staleness.

A buy at ask or sell at bid is normally marketable. It may be used with marketable-only but will be rejected/canceled with post-only.

## 6. Order lifecycle

States are `PENDING`, `ACCEPTED`, `TRIGGER_PENDING`, `WORKING`, `PARTIALLY_FILLED`, `FILLED`, `CANCELED`, `REJECTED`, and `EXPIRED`. Terminal states never transition.

- Market: attempts immediate execution, then cancels remainder. A protection price derived from `slippage_cap_bps` is mandatory when the dataset can expose unbounded gaps.
- Limit GTC: executes at the limit or better and otherwise rests.
- IOC: immediately fills available modeled liquidity, cancels remainder.
- FOK: fills the complete quantity immediately or rejects with no fill.
- Stop-market: remains trigger-pending; on trigger becomes a protected market order.
- Stop-limit: remains trigger-pending; on trigger becomes a limit order and may not fill.

Default trigger source is the mark price for liquidation and last eligible trade for player stops. Rulesets may choose index or quote triggers but must identify and persist the source.

For a sell stop, trigger when source is less than or equal to stop price. For a buy stop, trigger when greater than or equal. Trigger evaluation happens after the triggering market event is applied and before later sequence events.

## 7. Maker and taker

- `post_only=true` guarantees no immediate simulated execution. The default venue profile rejects a marketable order on acceptance. A future venue profile may cancel instead, but behavior is pinned per session.
- `marketable_only=true` is project terminology for IOC non-resting behavior. Limit orders fill only at their limit or better; the remainder cancels.
- Liquidity role is assigned per fill, not per order. A resting limit fill is maker only when the active fidelity model can justify it; otherwise it is `SIMULATED_MAKER` and the report is flagged.
- Maker rebates are negative fees and cannot cause accounting ambiguity.

## 8. Fidelity-dependent execution

### F0 — OHLCV bars

- Market order fills at the next eligible bar open plus configured slippage, never at the prior close.
- Limit/stop reach can use bar high/low, but ambiguous intrabar ordering is resolved by a pinned pessimistic, optimistic, or seeded bridge policy. Scored default is pessimistic.
- Maker/taker role, spread, queue, and partial depth are unknown. Post-only and marketable-only controls are disabled by default.
- Liquidation and stop ordering within one bar is flagged `INTRABAR_AMBIGUOUS`.

### F1 — trades plus BBO

- Market orders use current quotes with a configured size/slippage model.
- Resting limits become fill-eligible only after subsequent qualifying prints/quotes. Queue is modeled, not observed.
- Exact depth sweep and queue position are unavailable.

### F2 — L2/market-by-price

- Taker orders sweep displayed levels subject to market-impact policy.
- Resting queue-ahead is estimated from displayed size at arrival; cancels ahead use a pinned allocation model.
- Hidden orders and within-level identity remain unknown.

### F3 — L3/market-by-order

- Queue position uses individual order events when stable venue order IDs and sequencing exist.
- Execution remains counterfactual: the player's order was not in the historical book and cannot change later historical events. Size caps and uncertainty remain explicit.

No model fills against an event that precedes order acceptance.

## 9. Fees, funding, borrow, and corporate actions

- Fees round once per fill using settlement-currency minor units and venue-profile rounding.
- Scheduled funding is applied to the position held at the funding event using the latest revealed eligible funding rate. Missing funding data must be declared; zero is not silently assumed.
- Equity short borrow accrues only when a borrow model/rate exists; synthetic profiles may use an explicit fixed rate.
- Splits adjust position quantity, entry basis, price scale, and working orders without changing economic value. Cash dividends and futures variation settlement are explicit ledger events.
- Futures expiration/roll is never automatic unless a pinned continuous-contract policy describes the synthetic trade and costs. Prefer individual contracts for scored play.

## 10. Leverage changes

`SetLeverage` accepts values from 1× through 50× allowed by the session profile.

- Increasing requested leverage reduces required initial margin but does not create or resize a position.
- Decreasing leverage increases required margin. Reject with `INSUFFICIENT_AVAILABLE_MARGIN` if the new requirement cannot be met; do not liquidate merely because the user attempted an invalid change.
- A ruleset may cap leverage below 50× by instrument/venue. The setup UI shows both requested range and authentic cap.
- In `synthetic_isolated`, available margin equals isolated collateral minus initial margin reserved for positions and working orders.

## 11. Liquidation

After every price, fill, fee, funding, borrow, collateral, and margin-tier event:

1. Recompute mark, unrealized P&L, equity, maintenance requirement, and available margin.
2. If equity is at or below maintenance requirement, cancel working orders.
3. Submit a system reduce-only liquidation order using the fidelity-appropriate execution model and configured penalty.
4. Continue partial liquidation by tier if the profile supports it; v1 default fully closes.
5. If resulting equity is negative, record bankruptcy and clamp only displayable account balance according to the ruleset. Never erase deficit from the ledger.

The displayed liquidation price is an estimate because fees, funding, tier changes, and gaps can move the actual trigger.

## 12. Event precedence

Within equal `ts_event_ns`, order by provider sequence when reliable. Canonical priority for synthetic/scheduled ties is:

1. instrument definition/session-status change;
2. source market event in provider order;
3. triggers caused by that event;
4. fills/cancels caused by that event;
5. fee and realized-P&L ledger entries;
6. risk check and liquidation command;
7. scheduled funding/borrow event at its exact timestamp, followed by another risk check;
8. player command accepted at that logical instant according to recorded arrival sequence;
9. snapshot/output event.

Golden fixtures pin edge cases. An adapter may not reorder events to make fills convenient.

## 13. Accounting invariants

- Sum of ledger debits and credits is zero for every balanced transaction.
- Position change equals signed sum of fill quantities plus explicit corporate-action adjustments.
- Realized P&L is recognized only on closing quantity.
- Reversal resets entry basis only for the residual opposite position.
- Equity reconciles to cash plus marked positions within zero fixed-point atoms.
- Fees/funding/borrow appear exactly once.
- No terminal order fills later.
- A session cannot consume a market event beyond its reveal bound.

## 14. Score v1 proposal

Scoring is lexicographic, not a single seductive number:

1. `survived` (liquidated/bankrupt ranks below completed);
2. terminal return after all costs;
3. lower maximum drawdown;
4. lower peak effective leverage;
5. higher return over the unlevered same-instrument benchmark.

The UI may display an experimental composite, but challenges compare the versioned tuple. This avoids rewarding a lucky highly leveraged terminal outcome over survival quality.
