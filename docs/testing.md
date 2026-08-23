# Verification strategy

## 1. Test pyramid

- Schema tests: examples validate, invalid flag/price/unit combinations fail, generated models round-trip.
- Unit tests: fixed-point arithmetic, rounding, event ordering, order transitions, accounting, risk formulas, adapter parsers.
- Property tests: conservation and state-machine invariants over generated commands/events.
- Golden replay tests: canonical input streams and commands produce exact event JSONL and final hashes.
- Adapter contract tests: provider samples normalize identically and fail closed on schema drift.
- Integration tests: API, coordinator, sim-core, database, and visibility gate.
- Browser end-to-end tests: setup, trade, disconnect, liquidate, complete, review.
- Adversarial tests: future-data exfiltration and entitlement boundaries.
- Performance tests: event throughput, snapshot restore, chart query, websocket fan-out.

Networked provider tests are opt-in and never required for an ordinary pull request.

## 2. Required golden scenarios

Each scenario has a tiny synthetic market stream, instrument/ruleset, ordered commands, expected events, ledger, metrics, and final hash.

1. `long_partial_close`: buy, partial fill, sell less than position.
2. `sell_through_zero`: long 5, sell 8; close 5 and open short 3.
3. `buy_through_zero`: short 5, buy 8; close 5 and open long 3.
4. `reduce_only_excess`: long 5, reduce-only sell 8; fill at most 5 and cancel 3.
5. `reduce_only_race`: two reducing orders compete after one closes position.
6. `reverse_margin_partial`: close leg succeeds, residual open leg lacks margin.
7. `post_only_marketable`: buy at ask and sell at bid reject under default profile.
8. `marketable_only_partial`: sweep available modeled liquidity and cancel remainder.
9. `midpoint_rounding`: odd spread rounds toward passive side for buy and sell.
10. `stop_gap`: stop-market triggers through its level and fills at next executable prices.
11. `stop_limit_miss`: triggers during gap but remains unfilled beyond limit.
12. `ambiguous_bar`: stop and liquidation prices both occur within one F0 bar; pessimistic policy is pinned and warning emitted.
13. `l2_depth_sweep`: market order produces multiple price-level fills.
14. `maker_queue`: displayed queue ahead consumes before player fill.
15. `funding_liquidation`: funding charge crosses maintenance threshold at same timestamp.
16. `lower_leverage_reject`: invalid decrease leaves requested leverage/state unchanged.
17. `increase_leverage`: margin requirement falls while quantity/P&L remain unchanged.
18. `fee_rebate`: negative maker fee posts once and balances.
19. `futures_expiry`: settlement/expiry behavior uses instrument definition.
20. `split_working_order`: equity split adjusts position and working order economically.
21. `sequence_gap`: L2 interval quarantines or degrades; no depth fills after broken state.
22. `duplicate_command`: same idempotency/payload returns result; changed payload rejects.
23. `snapshot_resume`: uninterrupted and restored replay hashes match.
24. `integer_extremes`: near-bound values succeed or reject overflow deterministically.

## 3. Property invariants

Generate long command/event sequences and assert:

- ledger postings balance exactly;
- signed fill sum reconciles position;
- reduce-only never increases absolute exposure or changes sign;
- post-only has no taker/immediate fill;
- marketable-only never remains working;
- filled quantity never exceeds accepted quantity;
- terminal orders never transition or fill;
- event sequence and hash chain are continuous;
- identical input bytes yield identical output bytes across repeated runs;
- leverage change alone never changes cash, quantity, average entry, or P&L;
- unavailable margin cannot become negative without an explicit loss/charge event;
- no output causally references a market event after the reveal/input frontier.

## 4. Anti-lookahead suite

Run the cases in `anti-lookahead.md` against both HTTP and websocket transports. Use a canary future value/identifier in fixture data and scan all browser-visible bytes, logs returned to the user, exports, headers, ETags, source maps, and accessibility text. A canary occurrence before completion fails the build.

Test coarse candles whose interval crosses `revealed_through`; expected OHLCV derives only from revealed trades/bars and is marked incomplete.

## 5. Differential and metamorphic tests

- Compare Rust fixed-point calculations to a slow arbitrary-precision reference implementation in tests.
- Split one order into equivalent sequential fills at the same price and verify final economic state, while allowing distinct event history.
- Change playback speed and UI batching; canonical result must remain identical.
- Repartition the same canonical input without changing event order; replay result must remain identical.
- Round-trip canonical Parquet → event envelopes → Parquet and compare semantic hashes.

## 6. Adapter quality gates

For each partition:

- source checksum/size and decompression validation;
- parse count and rejected-row report;
- monotonic/order checks appropriate to source;
- duplicates and ID collisions;
- timestamps within requested interval and plausible resolution;
- prices/quantities aligned with point-in-time increments;
- trade/quote/book sanity, including crossed/locked/stale intervals;
- snapshot/delta continuity and recovery;
- derived bar reconciliation to trades where possible;
- coverage gaps and definition changes;
- deterministic rerun content hash.

## 7. Continuous integration gates

Every pull request runs formatting, linting, schema compatibility, unit/property tests with bounded cases, golden F0/F1 scenarios, API contract tests, and no-secret/no-large-data scans. Main/nightly additionally runs all golden tiers, larger property seeds, E2E browsers, mutation tests for critical accounting guards, replay determinism on supported platforms, and performance regression thresholds.

## 8. Release evidence

A release records exact commands, toolchain/lockfile hashes, passed scenario hashes, supported schema/ruleset versions, performance reference machine/results, known fidelity limitations, and any quarantined providers. No release is “green” if accounting, determinism, or lookahead tests are waived.
