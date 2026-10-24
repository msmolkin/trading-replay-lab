"""Entitlement-aware deterministic Tardis downloadable CSV adapter."""

from __future__ import annotations

import csv
import gzip
import io
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import date, timedelta
from decimal import Decimal, InvalidOperation
from pathlib import PurePosixPath
from typing import Protocol
from urllib.request import Request, urlopen

from trading_replay_ingest.core import FetchChunk, FetchPlan, FetchRequest, NormalizedBatch
from trading_replay_ingest.core.canonical import JsonValue

from .model import (
    ApiKeyProvider,
    TardisDataType,
    TardisEntitlement,
    TardisIntegrityError,
    TardisRequestError,
    TardisSchemaError,
)

DAY_NS = 86_400_000_000_000
MICROSECOND_TO_NS = 1_000
DATASET_BASE_URL = "https://datasets.tardis.dev/v1"


class TardisTransport(Protocol):
    """Small injectable boundary that keeps bearer secrets out of source references."""

    def get(self, url: str, max_bytes: int, bearer_token: str | None) -> bytes:
        """Fetch one bounded gzip object using an ephemeral bearer token when supplied."""
        ...


@dataclass(frozen=True, slots=True)
class UrlLibTardisTransport:
    """Minimal production HTTP transport with bounded reads."""

    timeout_seconds: float = 30.0

    def get(self, url: str, max_bytes: int, bearer_token: str | None) -> bytes:
        headers = {"User-Agent": "trading-replay-lab/1"}
        if bearer_token is not None:
            headers["Authorization"] = f"Bearer {bearer_token}"
        request = Request(url, headers=headers)
        with urlopen(request, timeout=self.timeout_seconds) as response:
            payload = bytes(response.read(max_bytes + 1))
        if len(payload) > max_bytes:
            raise TardisIntegrityError("Tardis dataset exceeds configured compressed byte ceiling")
        return payload


@dataclass(frozen=True, slots=True)
class TardisConfig:
    """Point-in-time normalization declaration for one Tardis dataset family."""

    exchange: str
    symbol: str
    data_type: TardisDataType
    instrument_id: str
    dataset_id: str
    price_scale: int
    qty_scale: int
    rate_scale: int = 12
    max_archive_bytes: int = 256 * 1024 * 1024
    max_uncompressed_bytes: int = 1024 * 1024 * 1024
    max_rows_per_chunk: int = 10_000_000
    estimated_cost_minor_per_chunk: int = 0

    def __post_init__(self) -> None:
        if not self.exchange or not self.instrument_id or not self.dataset_id:
            raise ValueError("exchange, instrument_id, and dataset_id are required")
        if not self.symbol or self.symbol != self.symbol.upper():
            raise ValueError("Tardis symbol must be non-empty uppercase text")
        if any(not 0 <= scale <= 18 for scale in (self.price_scale, self.qty_scale, self.rate_scale)):
            raise ValueError("decimal scales must be in [0, 18]")
        if self.max_archive_bytes <= 0 or self.max_uncompressed_bytes <= 0:
            raise ValueError("resource ceilings must be positive")
        if self.max_rows_per_chunk <= 0:
            raise ValueError("max_rows_per_chunk must be positive")
        if self.estimated_cost_minor_per_chunk < 0:
            raise ValueError("estimated provider cost cannot be negative")

    @property
    def url_symbol(self) -> str:
        """Return the documented uppercase URL-safe dataset symbol."""
        return self.symbol.replace("/", "-").replace(":", "-")

    @property
    def venue_id(self) -> str:
        """Stable canonical venue id derived from the Tardis exchange id."""
        return f"TARDIS:{self.exchange.upper()}"


class TardisAdapter:
    """Daily normalized CSV downloader with exact fixed-point normalization."""

    def __init__(
        self,
        config: TardisConfig,
        entitlement: TardisEntitlement,
        *,
        transport: TardisTransport | None = None,
        api_key_provider: ApiKeyProvider | None = None,
    ) -> None:
        if entitlement.coverage.exchange != config.exchange:
            raise ValueError("Tardis entitlement exchange does not match adapter config")
        if entitlement.coverage.symbol != config.symbol:
            raise ValueError("Tardis entitlement symbol does not match adapter config")
        self.config = config
        self.entitlement = entitlement
        self.transport = UrlLibTardisTransport() if transport is None else transport
        self.api_key_provider = api_key_provider

    def plan(self, request: FetchRequest) -> FetchPlan:
        """Build one independently resumable daily gzip object per covered UTC day."""
        if request.provider != "tardis":
            raise TardisRequestError("provider must be tardis")
        if request.instrument_id != self.config.instrument_id:
            raise TardisRequestError("request instrument does not match Tardis config")
        if request.dataset != self.config.data_type.value:
            raise TardisRequestError("request dataset does not match Tardis data type")
        if request.start_ns < 0 or request.end_ns <= request.start_ns:
            raise TardisRequestError("request range must be positive and non-empty")

        first_day = date(1970, 1, 1) + timedelta(days=request.start_ns // DAY_NS)
        last_day = date(1970, 1, 1) + timedelta(days=(request.end_ns - 1) // DAY_NS)
        count = (last_day - first_day).days + 1
        chunks: list[FetchChunk] = []
        for offset in range(count):
            day = first_day + timedelta(days=offset)
            if not self.entitlement.permits(self.config.data_type, day):
                raise TardisRequestError(
                    f"Tardis entitlement does not permit {self.config.data_type.value} on {day.isoformat()}"
                )
            chunks.append(
                FetchChunk(
                    key=f"{self.config.exchange}:{self.config.data_type.value}:{self.config.url_symbol}:{day.isoformat()}",
                    source_ref=self.dataset_url(day),
                    estimated_cost_minor=self.config.estimated_cost_minor_per_chunk,
                )
            )
        return FetchPlan(tuple(chunks))

    def dataset_url(self, day: date) -> str:
        """Build the documented daily datasets API URL without embedding credentials."""
        return (
            f"{DATASET_BASE_URL}/{self.config.exchange}/{self.config.data_type.value}/"
            f"{day:%Y/%m/%d}/{self.config.url_symbol}.csv.gz"
        )

    def fetch(self, chunk: FetchChunk) -> bytes:
        """Fetch one bounded gzip object; authenticated access is header-only and ephemeral."""
        self._validate_source_ref(chunk.source_ref)
        token = None if self.api_key_provider is None else self.api_key_provider()
        if not self.entitlement.sample_only and not token:
            raise TardisRequestError("authenticated Tardis entitlement requires an API key provider")
        return self.transport.get(chunk.source_ref, self.config.max_archive_bytes, token)

    def normalize(self, chunk: FetchChunk, raw: bytes) -> NormalizedBatch:
        """Normalize one gzip CSV object to canonical replay events."""
        self._validate_source_ref(chunk.source_ref)
        text = self._decompress(raw)
        reader = csv.DictReader(io.StringIO(text, newline=""))
        if reader.fieldnames is None:
            raise TardisSchemaError("Tardis CSV is missing a header")
        self._validate_header(tuple(reader.fieldnames))
        rows: list[dict[str, str]] = []
        for index, raw_row in enumerate(reader):
            if index >= self.config.max_rows_per_chunk:
                raise TardisIntegrityError("Tardis CSV exceeds configured row ceiling")
            if None in raw_row:
                raise TardisSchemaError("Tardis CSV row has more fields than the header")
            row = {key: value for key, value in raw_row.items() if key is not None and value is not None}
            self._validate_identity(row)
            rows.append(row)

        if self.config.data_type == TardisDataType.TRADES:
            events = [self._trade(row, index) for index, row in enumerate(rows)]
        elif self.config.data_type in (TardisDataType.QUOTES, TardisDataType.BOOK_TICKER):
            events = [self._bbo(row, index) for index, row in enumerate(rows)]
        elif self.config.data_type == TardisDataType.INCREMENTAL_BOOK_L2:
            events = self._incremental_l2(rows)
        elif self.config.data_type in (TardisDataType.BOOK_SNAPSHOT_25, TardisDataType.BOOK_SNAPSHOT_5):
            events = [self._snapshot_row(row, index) for index, row in enumerate(rows)]
        elif self.config.data_type == TardisDataType.DERIVATIVE_TICKER:
            events = self._derivative_events(rows)
        elif self.config.data_type == TardisDataType.LIQUIDATIONS:
            events = [self._liquidation(row, index) for index, row in enumerate(rows)]
        else:
            raise TardisRequestError("unsupported Tardis data type")
        return NormalizedBatch(tuple(events))

    def _trade(self, row: Mapping[str, str], index: int) -> dict[str, JsonValue]:
        side = _side(row["side"], allow_unknown=True)
        trade_id = row["id"]
        payload: dict[str, JsonValue] = {
            "price_atoms": str(_decimal_atoms(row["price"], self.config.price_scale, positive=True)),
            "qty_atoms": str(_decimal_atoms(row["amount"], self.config.qty_scale, positive=True)),
            "aggressor_side": side,
        }
        if trade_id:
            payload["trade_id"] = trade_id
        return self._event(row, index, "TRADE", payload, source_event_id=trade_id or None)

    def _bbo(self, row: Mapping[str, str], index: int) -> dict[str, JsonValue]:
        payload: dict[str, JsonValue] = {}
        if row["bid_price"]:
            payload["bid_price_atoms"] = str(
                _decimal_atoms(row["bid_price"], self.config.price_scale, positive=True)
            )
            payload["bid_qty_atoms"] = str(
                _decimal_atoms(row["bid_amount"], self.config.qty_scale, positive=False)
            )
        elif row["bid_amount"]:
            raise TardisSchemaError("bid amount is present without a bid price")
        if row["ask_price"]:
            payload["ask_price_atoms"] = str(
                _decimal_atoms(row["ask_price"], self.config.price_scale, positive=True)
            )
            payload["ask_qty_atoms"] = str(
                _decimal_atoms(row["ask_amount"], self.config.qty_scale, positive=False)
            )
        elif row["ask_amount"]:
            raise TardisSchemaError("ask amount is present without an ask price")
        if not payload:
            raise TardisSchemaError("Tardis quote has neither bid nor ask")
        return self._event(row, index, "BBO", payload)

    def _incremental_l2(self, rows: list[dict[str, str]]) -> list[dict[str, JsonValue]]:
        events: list[dict[str, JsonValue]] = []
        ready = False
        skipped_before_snapshot = False
        saw_incremental_after_snapshot = False
        index = 0
        while index < len(rows):
            local_timestamp = rows[index]["local_timestamp"]
            end = index + 1
            while end < len(rows) and rows[end]["local_timestamp"] == local_timestamp:
                end += 1
            group = rows[index:end]
            snapshot_flags = {_boolean(row["is_snapshot"]) for row in group}
            if len(snapshot_flags) != 1:
                raise TardisSchemaError("mixed snapshot and delta rows share one local timestamp")
            is_snapshot = snapshot_flags.pop()
            if is_snapshot:
                flags: list[str] = []
                if skipped_before_snapshot:
                    flags.append("PRE_SNAPSHOT_UPDATES_SKIPPED")
                if ready and saw_incremental_after_snapshot:
                    flags.append("RECONNECT_SNAPSHOT")
                events.append(self._snapshot_group(group, index, flags))
                ready = True
                saw_incremental_after_snapshot = False
            elif not ready:
                skipped_before_snapshot = True
            else:
                for row_offset, row in enumerate(group):
                    events.append(self._book_delta(row, index + row_offset))
                saw_incremental_after_snapshot = True
            index = end
        return events

    def _snapshot_group(
        self,
        rows: list[dict[str, str]],
        index: int,
        flags: list[str],
    ) -> dict[str, JsonValue]:
        bids: list[JsonValue] = []
        asks: list[JsonValue] = []
        for row in rows:
            amount = _decimal_atoms(row["amount"], self.config.qty_scale, positive=False)
            if amount == 0:
                continue
            level: dict[str, JsonValue] = {
                "price_atoms": str(_decimal_atoms(row["price"], self.config.price_scale, positive=True)),
                "qty_atoms": str(amount),
            }
            side = _side(row["side"], allow_unknown=False)
            (bids if side == "BUY" else asks).append(level)
        payload: dict[str, JsonValue] = {"bids": bids, "asks": asks, "scope": "FULL"}
        return self._event(rows[0], index, "BOOK_SNAPSHOT_L2", payload, quality_flags=flags)

    def _book_delta(self, row: Mapping[str, str], index: int) -> dict[str, JsonValue]:
        amount = _decimal_atoms(row["amount"], self.config.qty_scale, positive=False)
        payload: dict[str, JsonValue] = {
            "side": _side(row["side"], allow_unknown=False),
            "price_atoms": str(_decimal_atoms(row["price"], self.config.price_scale, positive=True)),
            "new_qty_atoms": str(amount),
            "action": "DELETE" if amount == 0 else "UPSERT",
        }
        return self._event(row, index, "BOOK_DELTA_L2", payload)

    def _snapshot_row(self, row: Mapping[str, str], index: int) -> dict[str, JsonValue]:
        depth = 25 if self.config.data_type == TardisDataType.BOOK_SNAPSHOT_25 else 5
        bids: list[JsonValue] = []
        asks: list[JsonValue] = []
        for level_index in range(depth):
            for prefix, target in (("bids", bids), ("asks", asks)):
                price = row[f"{prefix}[{level_index}].price"]
                amount = row[f"{prefix}[{level_index}].amount"]
                if not price and not amount:
                    continue
                if not price or not amount:
                    raise TardisSchemaError("partial Tardis snapshot level")
                qty = _decimal_atoms(amount, self.config.qty_scale, positive=False)
                if qty == 0:
                    continue
                target.append(
                    {
                        "price_atoms": str(
                            _decimal_atoms(price, self.config.price_scale, positive=True)
                        ),
                        "qty_atoms": str(qty),
                    }
                )
        payload: dict[str, JsonValue] = {
            "bids": bids,
            "asks": asks,
            "scope": "TOP_N",
            "depth": depth,
        }
        return self._event(row, index, "BOOK_SNAPSHOT_L2", payload)

    def _derivative_events(self, rows: list[dict[str, str]]) -> list[dict[str, JsonValue]]:
        output: list[dict[str, JsonValue]] = []
        for index, row in enumerate(rows):
            fields: tuple[tuple[str, str, int, str], ...] = (
                ("funding_rate", "FUNDING_RATE", self.config.rate_scale, "rate"),
                ("open_interest", "OPEN_INTEREST", self.config.qty_scale, "quantity"),
                ("index_price", "INDEX_PRICE", self.config.price_scale, "price"),
                ("mark_price", "MARK_PRICE", self.config.price_scale, "price"),
            )
            ordinal = 0
            for column, kind, scale, unit in fields:
                value = row[column]
                if not value:
                    continue
                output.append(
                    self._event(
                        row,
                        index * 8 + ordinal,
                        kind,
                        {"value_atoms": str(_decimal_atoms(value, scale, positive=False)), "unit": unit},
                    )
                )
                ordinal += 1
            funding_timestamp = row["funding_timestamp"]
            if funding_timestamp:
                timestamp_ns = _micros_to_ns(_canonical_uint(funding_timestamp))
                output.append(
                    self._event(
                        row,
                        index * 8 + ordinal,
                        "FUNDING_PAYMENT_TIME",
                        {"value_atoms": str(timestamp_ns), "unit": "ns"},
                    )
                )
        return output

    def _liquidation(self, row: Mapping[str, str], index: int) -> dict[str, JsonValue]:
        liquidation_id = row["id"]
        payload: dict[str, JsonValue] = {
            "price_atoms": str(_decimal_atoms(row["price"], self.config.price_scale, positive=True)),
            "qty_atoms": str(_decimal_atoms(row["amount"], self.config.qty_scale, positive=True)),
            "side": _side(row["side"], allow_unknown=True),
        }
        if liquidation_id:
            payload["liquidation_id"] = liquidation_id
        return self._event(
            row,
            index,
            "LIQUIDATION_PRINT",
            payload,
            source_event_id=liquidation_id or None,
        )

    def _event(
        self,
        row: Mapping[str, str],
        tie_breaker: int,
        kind: str,
        payload: dict[str, JsonValue],
        *,
        source_event_id: str | None = None,
        quality_flags: list[str] | None = None,
    ) -> dict[str, JsonValue]:
        timestamp_ns = _micros_to_ns(_canonical_uint(row["timestamp"]))
        local_ns = _micros_to_ns(_canonical_uint(row["local_timestamp"]))
        event: dict[str, JsonValue] = {
            "schema_version": "1.0.0",
            "dataset_id": self.config.dataset_id,
            "instrument_id": self.config.instrument_id,
            "venue_id": self.config.venue_id,
            "ts_event_ns": str(timestamp_ns),
            "ts_recv_ns": str(local_ns),
            "canonical_tie_breaker": str(tie_breaker),
            "kind": kind,
            "payload": payload,
            "quality_flags": [] if quality_flags is None else quality_flags,
        }
        event["source_event_id"] = source_event_id or (
            f"{self.config.data_type.value}:{row['local_timestamp']}:{tie_breaker}"
        )
        return event

    def _decompress(self, raw: bytes) -> str:
        try:
            with gzip.GzipFile(fileobj=io.BytesIO(raw), mode="rb") as archive:
                payload = archive.read(self.config.max_uncompressed_bytes + 1)
        except (OSError, EOFError) as error:
            raise TardisIntegrityError("invalid Tardis gzip dataset") from error
        if len(payload) > self.config.max_uncompressed_bytes:
            raise TardisIntegrityError("Tardis CSV exceeds configured uncompressed byte ceiling")
        try:
            return payload.decode("utf-8", errors="strict")
        except UnicodeDecodeError as error:
            raise TardisSchemaError("Tardis CSV must be UTF-8") from error

    def _validate_header(self, fieldnames: tuple[str, ...]) -> None:
        base = {"exchange", "symbol", "timestamp", "local_timestamp"}
        expected: set[str]
        if self.config.data_type in (TardisDataType.TRADES, TardisDataType.LIQUIDATIONS):
            expected = base | {"id", "side", "price", "amount"}
        elif self.config.data_type in (TardisDataType.QUOTES, TardisDataType.BOOK_TICKER):
            expected = base | {"ask_amount", "ask_price", "bid_price", "bid_amount"}
        elif self.config.data_type == TardisDataType.INCREMENTAL_BOOK_L2:
            expected = base | {"is_snapshot", "side", "price", "amount"}
        elif self.config.data_type in (TardisDataType.BOOK_SNAPSHOT_25, TardisDataType.BOOK_SNAPSHOT_5):
            depth = 25 if self.config.data_type == TardisDataType.BOOK_SNAPSHOT_25 else 5
            expected = set(base)
            for index in range(depth):
                expected.update(
                    {
                        f"asks[{index}].price",
                        f"asks[{index}].amount",
                        f"bids[{index}].price",
                        f"bids[{index}].amount",
                    }
                )
        elif self.config.data_type == TardisDataType.DERIVATIVE_TICKER:
            expected = base | {
                "funding_timestamp",
                "funding_rate",
                "predicted_funding_rate",
                "open_interest",
                "last_price",
                "index_price",
                "mark_price",
            }
        else:
            raise TardisRequestError("unsupported Tardis data type")
        if set(fieldnames) != expected:
            raise TardisSchemaError("Tardis CSV header does not match the documented dataset schema")

    def _validate_identity(self, row: Mapping[str, str]) -> None:
        if row["exchange"] != self.config.exchange:
            raise TardisSchemaError("Tardis row exchange does not match adapter configuration")
        if row["symbol"] != self.config.symbol:
            raise TardisSchemaError("Tardis row symbol does not match adapter configuration")

    def _validate_source_ref(self, source_ref: str) -> None:
        prefix = f"{DATASET_BASE_URL}/{self.config.exchange}/{self.config.data_type.value}/"
        if not source_ref.startswith(prefix):
            raise TardisRequestError("Tardis source reference is outside configured dataset path")
        if not source_ref.endswith(f"/{self.config.url_symbol}.csv.gz"):
            raise TardisRequestError("Tardis source reference symbol does not match config")
        path = PurePosixPath(source_ref.removeprefix(f"{DATASET_BASE_URL}/"))
        if ".." in path.parts:
            raise TardisRequestError("unsafe Tardis source reference")


def _canonical_uint(value: str) -> int:
    if not value or not value.isascii() or not value.isdigit() or (value.startswith("0") and value != "0"):
        raise TardisSchemaError("provider integer is not canonical unsigned decimal text")
    parsed = int(value)
    if parsed > 2**64 - 1:
        raise TardisSchemaError("provider integer exceeds uint64")
    return parsed


def _micros_to_ns(value: int) -> int:
    result = value * MICROSECOND_TO_NS
    if result > 2**63 - 1:
        raise TardisSchemaError("provider timestamp exceeds canonical int64 nanoseconds")
    return result


def _decimal_atoms(value: str, scale: int, *, positive: bool) -> int:
    if not value or value.strip() != value or value.startswith("+"):
        raise TardisSchemaError("provider decimal is not canonical")
    try:
        decimal = Decimal(value)
    except InvalidOperation as error:
        raise TardisSchemaError("provider decimal is malformed") from error
    if not decimal.is_finite():
        raise TardisSchemaError("provider decimal must be finite")
    scaled = decimal * (10**scale)
    integral = scaled.to_integral_value()
    if scaled != integral:
        raise TardisSchemaError("provider decimal exceeds declared instrument precision")
    atoms = int(integral)
    if atoms < -(2**63) or atoms > 2**63 - 1:
        raise TardisSchemaError("provider decimal exceeds canonical int64")
    if positive and atoms <= 0:
        raise TardisSchemaError("provider value must be positive")
    if not positive and atoms < 0:
        raise TardisSchemaError("provider quantity/value cannot be negative")
    return atoms


def _boolean(value: str) -> bool:
    if value == "true":
        return True
    if value == "false":
        return False
    raise TardisSchemaError("provider boolean must be true or false")


def _side(value: str, *, allow_unknown: bool) -> str:
    normalized = value.lower()
    if normalized == "buy" or normalized == "bid":
        return "BUY"
    if normalized == "sell" or normalized == "ask":
        return "SELL"
    if allow_unknown and normalized == "unknown":
        return "UNKNOWN"
    raise TardisSchemaError("provider side is invalid")
