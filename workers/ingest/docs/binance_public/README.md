# Binance Public Data adapter

This adapter reads Binance's public historical archive at `https://data.binance.vision`.
It never uses private account or trading endpoints and never requires an API credential.

## Supported source families

- Spot (`data/spot`)
- USD-M futures (`data/futures/um`)
- COIN-M futures (`data/futures/cm`)
- Daily `klines`, `trades`, and `aggTrades`

The adapter deliberately does **not** advertise BBO, L2, or MBO capability because those
streams are not present in these archive families. Klines declare simulator capability `F0`;
trade/aggregate-trade archives declare `F0T` only.

## Integrity and revisions

Every ZIP is fetched together with its sibling `.CHECKSUM`. The SHA-256 in that document
must name the exact ZIP and match its bytes before normalization. A caller can supply the
checksum previously recorded for a source URL; if Binance later republishes the same dated
object with a different checksum, ingestion fails with `ArchiveRevisionDetected` instead of
silently changing history. The new checksum can then be reviewed and catalogued as a new
manifest/version explicitly.

ZIPs must contain exactly one expected CSV member. Nested/traversal paths, extra members,
invalid UTF-8, and configured compressed/uncompressed size violations fail closed.

## Timestamp units

Binance's public-data documentation states that **Spot data from 2025-01-01 onward uses
microsecond timestamps**. Earlier Spot archives use milliseconds. The documented futures
archive examples use milliseconds. The adapter therefore chooses the unit from the archive
product and filename date; it never guesses based on the number of timestamp digits.

All timestamps are converted exactly to integer nanoseconds.

## Numeric normalization

Price and quantity scales are explicit adapter configuration derived from the point-in-time
instrument definition. Decimal source strings are converted with Python `Decimal`; a source
value that cannot be represented exactly at the declared scale is rejected. No price,
quantity, volume, or timestamp passes through binary floating point.

For trades, Binance's `isBuyerMaker=true` means the buyer supplied maker liquidity, so the
aggressor is normalized as `SELL`; `false` normalizes as `BUY`.

## Provenance

Primary provider documentation:

- <https://github.com/binance/binance-public-data>
- <https://data.binance.vision/>

The upstream public-data repository is MIT licensed. Dataset redistribution/export policy
must still be recorded by the catalog manifest rather than inferred from this adapter.
