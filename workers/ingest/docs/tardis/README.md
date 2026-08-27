# Tardis ingestion adapter

The Tardis adapter consumes the provider's normalized **downloadable CSV datasets**, not raw exchange-native replay messages. It deliberately treats provider coverage, local entitlement and ingestion budget as separate gates.

## Secrets and entitlement

`TardisConfig`, `FetchChunk`, checkpoint state and manifests never contain an API key. Authenticated downloads request a bearer token from the injected `ApiKeyProvider` only at fetch time and pass it to the HTTP `Authorization` header. Do not put tokens in dataset URLs, request options, logs, fixture files or repository configuration.

`TardisEntitlement.sample_only=true` permits only the first UTC day of each month, matching Tardis's public sample policy. Authenticated policies must explicitly enumerate allowed data types and coverage. The adapter fails during planning if any requested day is outside that policy or the point-in-time provider coverage record.

Every planned daily object carries `estimated_cost_minor`. The provider-neutral `BudgetGuard` rejects the full request/cost envelope before network access and accounts bytes/cost after each bounded fetch.

## Dataset mapping

| Tardis CSV | Canonical events | Replay use |
| --- | --- | --- |
| `trades` | `TRADE` | F0T; combines with BBO for F1 |
| `quotes` / `book_ticker` | `BBO` | F1 quote frontier |
| `incremental_book_L2` | `BOOK_SNAPSHOT_L2`, `BOOK_DELTA_L2` | F2 reconstruction inputs |
| `book_snapshot_5` / `book_snapshot_25` | `BOOK_SNAPSHOT_L2` (`TOP_N`) | bounded-depth snapshots only |
| `derivative_ticker` | `FUNDING_RATE`, `FUNDING_PAYMENT_TIME`, `OPEN_INTEREST`, `INDEX_PRICE`, `MARK_PRICE` | scheduled economics and marks |
| `liquidations` | `LIQUIDATION_PRINT` | historical liquidation prints |

Provider decimal strings are converted directly with `Decimal` into configured integer atoms. Floats never enter canonical output. Provider microsecond timestamps are checked and converted to nanoseconds. The row position within a daily CSV is the canonical tie breaker because Tardis documents CSV row order as original capture order when exchange timestamps tie.

## L2 reconstruction and reconnects

`incremental_book_L2` can begin with buffered non-snapshot updates received before the exchange's initial snapshot. The adapter discards those rows until the first `is_snapshot=true` message group and marks that snapshot `PRE_SNAPSHOT_UPDATES_SKIPPED`.

Rows sharing one `local_timestamp` form one provider message group. A snapshot group becomes one `BOOK_SNAPSHOT_L2` event. Once a snapshot is established, non-snapshot rows become absolute `BOOK_DELTA_L2` updates (`amount=0` is `DELETE`). A later snapshot after incremental updates is emitted as a fresh snapshot with `RECONNECT_SNAPSHOT`; consumers must discard prior book state.

Malformed mixed snapshot/delta groups, negative quantities, over-precision decimals, wrong exchange/symbol rows, unexpected CSV schemas and resource-limit violations fail closed.

## Capability policy

Capabilities are computed for a requested **interval**, not globally for an exchange:

- F2 requires `incremental_book_L2` entitlement/coverage for every UTC day in the interval. Snapshot-only files do not imply F2.
- F1 requires complete trades plus `quotes` or `book_ticker` coverage.
- F0T requires complete trades coverage.
- Funding and liquidation capabilities are independently derived from `derivative_ticker` and `liquidations` coverage.

A provider metadata response may advertise other Tardis data types. Unknown types are ignored for replay capability calculation rather than overstating fidelity.

## Verification workflow

Normal CI uses small gzip CSV fixtures and an injectable transport; it never calls Tardis or needs a credential. The smoke tests cover deterministic planning, public-sample denial, exact normalization, L2 pre-snapshot/reconnect behavior, derivative economics, liquidation mapping and interrupted-run resume. For a manual licensed smoke test, obtain current `/v1/exchanges/:exchange` metadata, construct `TardisCoverage`, inject a secret provider from the local environment/key store, and run the normal ingestion runner with explicit request/byte/cost budgets.
