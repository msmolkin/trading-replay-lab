# Product specification

Status: implementation baseline

Last updated: 2026-08-23

## 1. Problem

Most backtests test a completed strategy against visible history. Trading Replay Lab tests a human decision process against an unrevealed historical path. The player must size, manage margin, and react as events become visible. Results must distinguish skill, favorable path, fill-model assumptions, and survival.

## 2. Goals

- Replay any eligible historical interval supported by installed data.
- Support crypto first, followed by futures and US equities through adapters.
- Let a player trade signed exposure with 1×–50× requested leverage.
- Support realistic order intent: buy, sell, short, cover, reverse, reduce-only, post-only, marketable-only, limit, market, stop-market, and stop-limit.
- Let a sell or buy cross zero in one atomic command when not reduce-only.
- Use order-flow data when available and visibly degrade when it is not.
- Prevent the client from learning the unrevealed path in blind modes.
- Make every run deterministic, auditable, exportable, and comparable.
- Show path-sensitive risk: drawdown, margin utilization, liquidation proximity, and ruin—not only terminal return.

## 3. Non-goals for v1

- Sending orders to a real broker or exchange.
- Claiming that simulated profitability predicts future profitability.
- Portfolio margin, options, multi-leg spreads, or cross-venue arbitrage.
- Perfect reconstruction of hidden liquidity, matching-engine latency, or the market impact of hypothetical large orders.
- Redistributing proprietary market data.
- Multiplayer synchronous competition.

## 4. Personas and primary flow

The initial persona is an individual trader testing discretionary decisions.

1. Choose mode, asset class, instrument or eligible universe, initial equity, episode duration, and ruleset.
2. Choose information policy:
   - exact visible start;
   - coarse-selection chart with fine history sealed;
   - server-selected random episode.
3. Review a preflight card: data source, fidelity tier, known gaps, fees, funding/borrow treatment, leverage regime, and enabled order behaviors.
4. Commit setup. The episode identity and future events become immutable.
5. Advance the event clock, inspect only revealed charts/order flow, and submit commands.
6. End voluntarily, reach episode end, or liquidate.
7. Reveal the complete episode and review decisions, alternate benchmarks, uncertainty, and ledger proof.

## 5. Session modes

### Practice

Instrument, venue, calendar time, and all already-historical context are visible. Future events within the episode remain gated.

### Blind window

The setup service exposes only the permitted coarse aggregates, such as weekly or monthly bars. The player may select a coarse bucket or an exact time using a date control. Once committed, lower granularity before the chosen start is no longer queryable through the session. The UI may replace absolute calendar labels with relative elapsed time.

### Random sealed

The player declares constraints, the server commits the eligible-set hash, and a cryptographically secure draw selects one episode. Instrument identity and calendar time may be independently hidden until completion.

### Challenge

A challenge pins a public ruleset, eligible-set commitment, duration, seed policy, and scoring version. Each entrant receives an independently selected episode unless the challenge explicitly uses a shared episode after entries are locked.

## 6. Setup options

- Asset class: crypto spot, crypto perpetual, dated futures, US equity.
- Venue and instrument, or an eligible universe.
- Episode start and duration.
- Initial cash/equity and settlement currency.
- Requested leverage: integer or decimal from 1× through 50×.
- Margin mode: `synthetic_isolated` for v1; authentic instrument profiles later.
- Starting chart visibility and maximum in-game chart granularity.
- Playback speed and whether pause is allowed.
- Commission, maker rebate, taker fee, spread/fill model, slippage model.
- Funding, borrow, interest, corporate-action, and futures-roll treatment.
- Order-flow panel visibility.
- End condition and score version.

Setup validation rejects combinations not supported by the dataset/ruleset. It does not silently substitute a weaker model.

## 7. Trading controls

The ticket exposes intent presets but submits one canonical order command.

| Control | Canonical effect |
|---|---|
| Buy / long | Positive quantity delta; may cover a short first |
| Sell | Negative quantity delta; may close long and open short |
| Short | Negative delta with UI validation that intended result is short |
| Cover | Positive delta, reduce-only by default |
| Reverse | Target position is the opposite sign; one atomic order may produce close and open fill legs |
| Close | Target position zero; reduce-only |
| Reduce-only | May reduce absolute exposure only; excess cannot cross zero |
| Maker-only | `post_only=true`; rejects/cancels if immediately executable |
| Taker-only | `marketable_only=true`; IOC semantics, never rests |
| Bid / ask / midpoint | Snapshots a limit price from currently revealed reference quotes |
| Market | Consumes the executable model immediately up to protection limits |
| Stop loss | Creates stop-market or stop-limit trigger; it is not itself a price source |

Detailed precedence, crossing, and fill rules are normative in `trading-rules.md`.

## 8. Market display

- Candles at allowed intervals, with unrevealed candle portions absent rather than masked client-side.
- Last trade, BBO, spread, depth, tape, funding, open interest, and liquidation feed only when present.
- Position, average entry, notional, requested/effective leverage, initial/maintenance margin, liquidation estimate, cash, equity, available margin, realized/unrealized P&L, fees, funding, borrow, and drawdown.
- Working orders and ordered audit feed with acceptance, trigger, partial-fill, fill, cancel, reject, expire, and liquidation events.
- Persistent fidelity badge and warnings for gaps or approximate fills.

## 9. Playback

- Commands enter the simulation at a deterministic event timestamp and sequence.
- Speeds include step, 1×, and accelerated playback; acceleration changes presentation, never event ordering.
- Pause stops reveal progression but not already ordered processing at the current logical timestamp.
- The player cannot rewind an active scored session. Practice forks are separate sessions with a visible `forked` label.
- No jumping forward and then returning. A skip permanently reveals and processes the skipped interval.
- If there are no market events, time advances to the next scheduled event, funding event, session boundary, or episode end.

## 10. End-of-run report

The report includes:

- terminal equity and return;
- realized/unrealized P&L, fees, funding, borrow, and liquidation penalties;
- maximum drawdown, peak margin utilization, minimum liquidation buffer, turnover, exposure-time distribution, and survival;
- time-weighted and money-weighted return when applicable;
- unlevered buy-and-hold and cash benchmarks over the same revealed episode;
- a decision timeline over the now-revealed full path;
- data/fill fidelity and an uncertainty statement;
- immutable setup, simulator version, dataset manifest hashes, command log hash, event log hash, and result hash.

Scores rank survival first, then risk-adjusted return. The versioned score must not change after a challenge begins. A proposed v1 score is documented in `trading-rules.md`.

## 11. Functional acceptance criteria

1. A fixture episode can be started in every mode without sending an event after `revealed_through` to the browser.
2. A player can increase or decrease leverage within 1×–50×, with deterministic acceptance/rejection and margin effects.
3. A sell larger than a long position closes the long and opens the residual short, producing auditable fill legs and correct fees.
4. The same command with reduce-only enabled cannot cross zero.
5. Post-only and marketable-only are mutually exclusive and obey their guarantees.
6. Bid, ask, and midpoint selectors use the quote visible when the command is accepted; the snapped integer price is recorded.
7. Market, stop-market, and stop-limit orders behave according to the declared fidelity tier.
8. Partial fills and multiple fills maintain accounting conservation.
9. Funding/borrow and liquidation are processed in deterministic event order.
10. Replaying the same manifest, ruleset, seed, and commands produces byte-identical canonical events and result hash on supported platforms.
11. Missing/gapped data pauses or degrades according to policy; it never fabricates an exact result.
12. The run export is sufficient for an offline verifier without including licensed source data.

## 12. Success metrics

- Zero verified lookahead leaks in automated security scenarios.
- Zero accounting invariant failures across property tests and golden fixtures.
- A new bar/trade provider adapter can be implemented without modifying sim-core.
- A local user can run a complete free-data crypto episode after one documented setup flow.
- At least 90% of completed runs expose no unexplained data-quality warning.
- Median command-to-state-update latency under 100 ms at normal playback for cached single-instrument episodes; accelerated replay throughput is benchmarked separately.

## 13. Open product decisions

These are intentionally deferred and must become ADRs before their milestone:

- Whether shared challenges reveal the same episode to all players or sample independently.
- Whether hidden instrument identity is a first-class blind option.
- Default market-impact caps by asset class.
- Authentic cross-margin and portfolio support.
- Whether a hosted verifier is operated by the project or only specified.
