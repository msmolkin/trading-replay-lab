# Data provider and adapter strategy

Provider capabilities, prices, entitlements, and terms change. This survey was checked against official documentation on 2026-08-23; adapters must discover capabilities at runtime and pin observed coverage in manifests.

## 1. Selection principles

- Prefer bulk archives for reproducible history and checksum verification.
- Prefer exchange-native IDs/timestamps and retain source provenance.
- Separate provider download from canonical normalization.
- A common adapter library is not evidence that deep history exists.
- Never infer order-book history from a current order-book endpoint.
- Plan cost and entitlement before download.
- Do not redistribute raw data unless provider and venue terms clearly allow it.

## 2. Initial matrix

| Adapter | Assets | Likely fidelity | Cost/access | Role |
|---|---|---:|---|---|
| Binance Public Data | Spot, USD-M and COIN-M crypto futures | F0, plus trades (`F0T`) | Public downloadable daily/monthly files | Free default crypto MVP |
| Tardis.dev | Crypto across many exchanges | F1–F2 and sometimes exchange-native deeper data | Commercial; sample access exists | High-fidelity crypto replay |
| CCXT | Many crypto exchanges | Capability-dependent F0/recent trades | Open-source library; exchange API limits apply | Discovery/API fallback, not archival guarantee |
| Databento | Futures, equities, options and other venues | F0–F3 depending dataset/schema | Metered and license-dependent | Primary futures/equity high-fidelity adapter |
| Alpaca Market Data | US stocks, crypto, options | Bars/trades/quotes by feed/plan | Credential/plan and feed dependent | Accessible equities/crypto fallback |
| Canonical file | Any | Declared by manifest | User-supplied Parquet/CSV | Escape hatch and test/import path |

### Binance Public Data

The official [Binance Public Data repository](https://github.com/binance/binance-public-data) documents daily/monthly archives for all symbols, spot aggregate trades, klines, trades, USD-M futures, COIN-M futures, checksums, and archive replacements. It also notes a spot timestamp-unit change beginning in 2025. The adapter must inspect schema by product/date rather than hard-code milliseconds.

This source is excellent for free bar and trade replay. Its documented archive fields do not provide the historical BBO/depth stream needed for F1/F2 claims, so the game must use an explicit spread/slippage model or pair it with another lawful source.

### Tardis.dev

Official [Tardis.dev data documentation](https://docs.tardis.dev/faq/data) describes raw tick-level trades, streaming order-book snapshots plus incremental updates, funding, liquidations, and client-side reconstructed snapshots. Its [API quickstart](https://docs.tardis.dev/api/quickstart) covers historical replay, normalization, and order-book reconstruction.

The adapter should ingest raw exchange-native data when maximum fidelity is required, or normalized CSV for faster delivery. Reconstruction must begin from a valid snapshot and quarantine sequence gaps until the next valid snapshot. Exact fields and coverage differ by exchange/instrument/date.

### CCXT

The [CCXT manual](https://github.com/ccxt/ccxt/wiki/Manual) exposes unified `fetchOHLCV`, `fetchTrades`, and order-book methods, while warning that historical pagination and support are exchange-specific and many endpoints return only recent history. Use CCXT to accelerate catalog and shallow API adapters, but store the exact exchange capability result and never promise “any time in history” from the unified method alone.

### Databento

Databento's official [schemas documentation](https://databento.com/docs/knowledge-base) lists OHLCV, tick trades, BBO/TBBO, market-by-price, market-by-order, instrument definitions, statistics, status, and corporate actions. Its [historical API](https://databento.com/docs/api-reference-historical) supports programmatic/batch access, and its [MBO documentation](https://databento.com/docs/schemas-and-data-formats/mbo) explains individual order events and snapshots.

This is the preferred route for futures/equity depth and queue research, but cost and venue licensing must be treated as first-class setup constraints. Futures should use point-in-time definitions and individual contracts for scored play; continuous symbols require an explicit roll policy.

### Alpaca

The official [Alpaca Historical API](https://docs.alpaca.markets/us/docs/historical-api) covers stocks, crypto, options, and news; the stock API exposes historical trades as well as bars/quotes through plan-specific feeds. The adapter must record selected feed (for example IEX versus SIP where applicable), entitlement, coverage, and adjustment behavior. It is a fallback for accessible equity data, not a substitute for MBO.

## 3. Provider rollout

1. Canonical-file adapter and synthetic fixtures: validates contracts without network/data terms.
2. Binance Public Data bars and trades: produces a completely free crypto path.
3. Tardis adapter: enables BBO/L2/order-flow play for selected crypto venues.
4. Databento adapter: adds individual futures contracts, equities, definitions, sessions, and higher fidelity.
5. Alpaca adapter: broadens accessible equities/crypto bar/trade/quote coverage.
6. Additional exchange archives: accept only with official source documentation, stable checksums or equivalent integrity, and a maintenance owner.

## 4. Coverage catalog

The application does not advertise global vague coverage. It materializes rows keyed by provider, dataset/feed, venue, canonical instrument, event kind, start/end, gaps, timestamp resolution, fidelity, adapter version, entitlement principal, and manifest status.

“Playable at any time” means any start whose full requested episode plus warm-up lies within a currently eligible coverage segment. The setup API returns exact eligible intervals.

## 5. Order-flow requirements

True order-flow playback requires:

- an initial book snapshot with defined scope;
- contiguous incremental sequence or a source-specific recovery rule;
- trade/book ordering semantics;
- side, price, quantity, and update action;
- instrument tick/lot definitions effective at the event time;
- documented handling of book clears, reconnects, and crossed/locked books.

Without these, keep trades/tape if useful but do not construct a trustworthy book. A snapshot sampled every N seconds is not equivalent to incremental depth.

## 6. Revisions and reproducibility

Archives can replace prior files. Download checksums and source modification metadata, content-address raw objects, and never mutate an existing canonical dataset ID. A replacement creates a new manifest/data version; old completed runs retain their original hashes or become explicitly unverifiable if lawful source access disappears.

## 7. Licensing guardrails

- Repository fixtures must be synthetic, generated, public-domain, or accompanied by clear redistribution permission and provenance.
- User-provided credentials and downloads remain outside Git and CI.
- Hosted mode enforces provider entitlements per user and avoids cross-user cache exposure where terms prohibit it.
- Exports default to hashes, derived player events, and limited display artifacts—not raw provider rows.
- Each adapter has a maintainer-reviewed `LEGAL.md` before production enablement. It records links and operational restrictions but is not legal advice.
- Provider names and logos are descriptive; do not imply endorsement.

## 8. Adapter acceptance checklist

- Capability discovery and exact coverage query.
- Restartable download with rate limiting, retries, checksums, and cost guard.
- Unit/date-specific timestamp parsing.
- Point-in-time instrument definitions.
- Deterministic normalization and deduplication.
- Sequence/gap/book validation appropriate to claimed tier.
- Provenance-rich immutable manifest.
- Contract fixtures without secrets or restricted data.
- Tests for schema drift and a fail-closed unknown format.
- Documentation of terms, entitlement, known limitations, and revocation path.
