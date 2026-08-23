# Canonical data contracts

This document defines the semantic contract. Machine-readable schemas begin in `schemas/` and move to `packages/contracts` in task M0-03.

## 1. Numeric representation

- Price is signed `int64 price_atoms` plus `price_scale` (decimal places) from instrument definition.
- Quantity is signed/unsigned `int64 qty_atoms` plus `qty_scale`.
- Monetary ledger amounts are signed `int64 amount_minor` in the currency's declared scale; implementations must reject overflow.
- Rates use signed parts-per-billion (`rate_ppb`) unless a schema explicitly declares another integer unit.
- Timestamps are signed UTC epoch nanoseconds (`*_ns`).
- JSON transports integers outside JavaScript's safe range as base-10 strings. Generated TypeScript models expose `bigint` at the domain boundary.

Float fields are forbidden in canonical schemas.

## 2. Instrument definition

Required fields include stable canonical ID, provider/venue symbols, asset class, product type, base/quote/settlement currencies, tick and quantity increments, price/quantity scales, contract multiplier, linear/inverse settlement, session calendar, listing/expiry, and definition-effective interval.

Identifiers are time-aware. A ticker string alone is never a stable equity or futures identity.

## 3. Market event envelope

```text
MarketEvent {
  schema_version,
  dataset_id,
  instrument_id,
  venue_id,
  ts_event_ns,
  ts_recv_ns?,
  source_sequence?,
  canonical_tie_breaker,
  source_event_id?,
  kind,
  payload,
  quality_flags[]
}
```

Kinds:

- `TRADE`: price, quantity, aggressor side if known, trade ID.
- `BBO`: bid/ask price and size; either side may be absent during transitions.
- `BOOK_SNAPSHOT_L2`: ordered levels, snapshot scope/depth.
- `BOOK_DELTA_L2`: side, price, new aggregate quantity, action semantics.
- `ORDER_EVENT_L3`: order ID, side, price, quantity, add/modify/cancel/fill/clear action.
- `BAR`: interval, open/high/low/close, base/quote volume, trade count when known, completeness.
- `MARK_PRICE`, `INDEX_PRICE`, `FUNDING_RATE`, `FUNDING_PAYMENT_TIME`, `OPEN_INTEREST`, `LIQUIDATION_PRINT`.
- `DEFINITION`, `CORPORATE_ACTION`, `SESSION_STATUS`, `SETTLEMENT`.

Unknown aggressor side is `UNKNOWN`, never guessed without a derived-field provenance marker.

## 4. Capabilities and fidelity

`DataCapabilities` is computed per instrument **and interval**, not only per provider:

```text
has_bars(intervals[])
has_trades
has_bbo
has_l2_snapshots
has_l2_deltas
has_l3
has_mark/index/funding/open_interest/liquidations
source_start_ns / source_end_ns
known_gaps[]
timestamp_resolution_ns
sequence_quality
redistribution_class
```

Execution tier is the highest fully validated tier:

| Tier | Minimum | Supported claims |
|---|---|---|
| F0 | complete OHLCV interval | bar-path approximation only |
| F1 | ordered trades + BBO | observed spread and print-driven approximate resting fills |
| F2 | L2 snapshot + contiguous deltas | displayed-depth sweep and price-level queue model |
| F3 | sequenced MBO/L3 | individual visible-order queue reconstruction |

Trades without BBO are `F0T`, a useful ingestion capability but not sufficient for F1 execution claims. A gap downgrades or splits eligibility until a new trustworthy snapshot.

## 5. Bars and aggregation

- Source bars and project-derived bars are distinguished.
- Derived bars are built only from canonical trades and record input partition hashes.
- `complete=false` for a currently forming or gap-affected interval.
- Empty time intervals are absent unless the venue defines an official zero-volume bar.
- Calendar bars declare timezone/session rules. Weekly/monthly equity and futures bars are not naive UTC buckets.
- Visibility-gated aggregation clamps inputs at `revealed_through`; it never returns a final high/low/close for a partially revealed candle.

## 6. Dataset manifest

Every eligible partition has a manifest containing:

- provider, dataset, adapter and canonical schema versions;
- venue/instrument definition hashes;
- requested and actual coverage bounds;
- source object URI identifiers without credentials;
- source and canonical content hashes;
- row counts by kind;
- min/max event timestamp and sequence;
- duplicates removed and exact policy;
- gaps, crossed-book intervals, stale-quote intervals, outliers, clock corrections, and degradation decisions;
- redistribution/licensing classification;
- ingestion time and tool build identity.

A manifest status is `PENDING`, `VALID`, `DEGRADED`, `QUARANTINED`, or `REVOKED`. Only ruleset-compatible `VALID`/explicitly accepted `DEGRADED` partitions can enter the episode catalog.

## 7. Session command envelope

```text
CommandEnvelope {
  schema_version,
  command_id,
  idempotency_key,
  session_id,
  principal_id,
  accepted_at_ns,
  logical_ts_ns,
  arrival_seq,
  expected_session_version,
  payload,
  payload_hash
}
```

Repeated idempotency keys with identical payload return the original result. Reuse with a different payload rejects.

## 8. Domain events and ledger

Every domain event includes schema version, session ID, `event_seq`, logical timestamp, causation ID, correlation ID, payload, prior event hash, and current event hash.

Fills contain order ID, execution ID, signed quantity, price, liquidity role (`MAKER`, `TAKER`, `SIMULATED_MAKER`, `UNKNOWN`), model/version, source market-event references, fee, and uncertainty flags.

Ledger transactions are balanced collections of postings. Minimum accounts include cash, position cost basis, realized P&L, unrealized P&L read-model adjustment, fees, funding, borrow, dividends, margin collateral, liquidation penalty, and deficit. Unrealized values must not be mistaken for cash postings.

## 9. Session visibility

```text
SessionVisibility {
  phase: SETUP | ACTIVE | COMPLETED,
  revealed_through_ns,
  permitted_intervals[],
  identity_visibility,
  calendar_visibility,
  order_flow_visibility,
  generation
}
```

Every player-facing market response includes `as_of_ns`, `revealed_through_ns`, completeness, source/fidelity label, and visibility generation. Completed sessions may transition to full reveal through one append-only event.

## 10. Result/proof bundle

The portable bundle includes setup/ruleset, commitments and revealed nonces, manifest hashes (not necessarily data), ordered commands, canonical domain events, periodic state hashes, final metrics, and result hash. An offline verifier given lawful access to matching market partitions can reproduce every event.

## 11. Compatibility rules

- Adding an optional field is minor-compatible.
- Changing numeric unit, rounding, required field, enum behavior, or event ordering is breaking.
- Readers reject unsupported major versions and preserve unknown optional fields when proxying.
- Golden fixtures state exact schema/ruleset/simulator versions.
