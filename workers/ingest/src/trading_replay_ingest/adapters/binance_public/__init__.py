# ruff: noqa: I001  # Ruff 0.16.2 misclassifies this otherwise stable import block.

"""Deterministic Binance Public Data archive adapter.

The official archive publishes immutable-looking date-addressed ZIP objects plus sibling
SHA-256 checksum files. Because Binance documents that archives can later be replaced,
this adapter treats checksum changes as explicit revisions instead of silently accepting
them under the same source URL.
"""

from __future__ import annotations

import csv
import hashlib
import io
import zipfile
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import date, timedelta
from decimal import Decimal, InvalidOperation
from enum import StrEnum
from pathlib import PurePosixPath
from typing import Protocol
from urllib.request import Request, urlopen

from trading_replay_ingest.core import FetchChunk, FetchPlan, FetchRequest, NormalizedBatch
from trading_replay_ingest.core.canonical import JsonValue


DAY_NS = 86_400_000_000_000
SPOT_MICROSECOND_START = date(2025, 1, 1)
BASE_URL = "https://data.binance.vision"


class BinanceProduct(StrEnum):
    """Public archive product families."""

    SPOT = "spot"
    USD_M = "um"
    COIN_M = "cm"


class BinanceDataKind(StrEnum):
    """Archive data kinds supported without overstating fidelity."""

    KLINES = "klines"
    TRADES = "trades"
    AGG_TRADES = "aggTrades"


class BinancePublicError(ValueError):
    """Base class for deterministic provider failures."""


class BinanceRequestError(BinancePublicError):
    """Request cannot be represented by this configured adapter."""


class BinanceIntegrityError(BinancePublicError):
    """Checksum or archive structure failed validation."""


class ArchiveRevisionDetected(BinanceIntegrityError):
    """A date-addressed official object changed from a previously recorded checksum."""


class BinanceSchemaError(BinancePublicError):
    """Official archive row violates the declared schema/precision."""


class HttpTransport(Protocol):
    """Small injectable byte-fetch boundary for deterministic recorded tests."""

    def get(self, url: str, max_bytes: int) -> bytes:
        """Fetch at most `max_bytes`, failing if the response is larger."""
        ...


@dataclass(frozen=True, slots=True)
class UrlLibTransport:
    """Minimal production transport with bounded reads and no credentials."""

    timeout_seconds: float = 30.0

    def get(self, url: str, max_bytes: int) -> bytes:
        request = Request(url, headers={"User-Agent": "trading-replay-lab/1"})
        with urlopen(request, timeout=self.timeout_seconds) as response:
            payload = bytes(response.read(max_bytes + 1))
        if len(payload) > max_bytes:
            raise BinanceIntegrityError("provider object exceeds configured byte ceiling")
        return payload


@dataclass(frozen=True, slots=True)
class BinancePublicConfig:
    """Point-in-time normalization declaration for one symbol/archive family."""

    product: BinanceProduct
    data_kind: BinanceDataKind
    symbol: str
    instrument_id: str
    dataset_id: str
    price_scale: int
    qty_scale: int
    interval: str | None = None
    max_archive_bytes: int = 128 * 1024 * 1024
    max_uncompressed_bytes: int = 512 * 1024 * 1024

    def __post_init__(self) -> None:
        if not self.symbol or self.symbol != self.symbol.upper():
            raise ValueError("Binance symbol must be non-empty uppercase text")
        if not self.instrument_id or not self.dataset_id:
            raise ValueError("instrument_id and dataset_id are required")
        if not 0 <= self.price_scale <= 18 or not 0 <= self.qty_scale <= 18:
            raise ValueError("decimal scales must be in [0, 18]")
        if self.data_kind == BinanceDataKind.KLINES and not self.interval:
            raise ValueError("kline interval is required")
        if self.data_kind != BinanceDataKind.KLINES and self.interval is not None:
            raise ValueError("interval is only valid for klines")
        if self.max_archive_bytes <= 0 or self.max_uncompressed_bytes <= 0:
            raise ValueError("resource ceilings must be positive")

    @property
    def capabilities(self) -> tuple[str, ...]:
        """Conservative simulator-fidelity capability declaration."""
        if self.data_kind == BinanceDataKind.KLINES:
            return ("F0",)
        return ("F0T",)

    @property
    def venue_id(self) -> str:
        return {
            BinanceProduct.SPOT: "BINANCE_SPOT",
            BinanceProduct.USD_M: "BINANCE_USDM",
            BinanceProduct.COIN_M: "BINANCE_COINM",
        }[self.product]


class BinancePublicAdapter:
    """Verified daily archive downloader and canonical normalizer."""

    def __init__(
        self,
        config: BinancePublicConfig,
        *,
        transport: HttpTransport | None = None,
        known_checksums: Mapping[str, str] | None = None,
    ) -> None:
        self.config = config
        self.transport = UrlLibTransport() if transport is None else transport
        self.known_checksums = {} if known_checksums is None else dict(known_checksums)
        self._observed_checksums: dict[str, str] = {}

    def plan(self, request: FetchRequest) -> FetchPlan:
        """Plan one independently verifiable official daily archive per UTC date."""
        if request.provider != "binance_public":
            raise BinanceRequestError("provider must be binance_public")
        if request.instrument_id != self.config.instrument_id:
            raise BinanceRequestError("request instrument does not match adapter configuration")
        if request.dataset != self.config.data_kind.value:
            raise BinanceRequestError("request dataset does not match adapter data kind")
        if request.start_ns < 0 or request.end_ns <= request.start_ns:
            raise BinanceRequestError("request range must be positive and non-empty")

        first_day = date(1970, 1, 1) + timedelta(days=request.start_ns // DAY_NS)
        last_day = date(1970, 1, 1) + timedelta(days=(request.end_ns - 1) // DAY_NS)
        count = (last_day - first_day).days + 1
        chunks = tuple(
            FetchChunk(
                key=f"{self.config.product.value}:{self.config.data_kind.value}:{day.isoformat()}",
                source_ref=self.archive_url(day),
                estimated_cost_minor=0,
            )
            for offset in range(count)
            for day in (first_day + timedelta(days=offset),)
        )
        return FetchPlan(chunks)

    def archive_url(self, archive_day: date) -> str:
        """Build the documented daily archive URL deterministically."""
        product_path = (
            "spot"
            if self.config.product == BinanceProduct.SPOT
            else f"futures/{self.config.product.value}"
        )
        day_text = archive_day.isoformat()
        if self.config.data_kind == BinanceDataKind.KLINES:
            assert self.config.interval is not None
            filename = f"{self.config.symbol}-{self.config.interval}-{day_text}.zip"
            suffix = f"klines/{self.config.symbol}/{self.config.interval}/{filename}"
        else:
            kind = self.config.data_kind.value
            filename = f"{self.config.symbol}-{kind}-{day_text}.zip"
            suffix = f"{kind}/{self.config.symbol}/{filename}"
        return f"{BASE_URL}/data/{product_path}/daily/{suffix}"

    def fetch(self, chunk: FetchChunk) -> bytes:
        """Fetch an archive and verify its sibling SHA-256 before returning bytes."""
        self._validate_source_ref(chunk.source_ref)
        raw = self.transport.get(chunk.source_ref, self.config.max_archive_bytes)
        checksum_document = self.transport.get(f"{chunk.source_ref}.CHECKSUM", 4096)
        expected = self._parse_checksum(chunk.source_ref, checksum_document)
        actual = hashlib.sha256(raw).hexdigest()
        if actual != expected:
            raise BinanceIntegrityError("official archive SHA-256 does not match CHECKSUM")
        previous = self.known_checksums.get(chunk.source_ref)
        if previous is not None and previous != expected:
            raise ArchiveRevisionDetected(
                f"official archive revision detected for {PurePosixPath(chunk.source_ref).name}"
            )
        self._observed_checksums[chunk.source_ref] = expected
        return raw

    def observed_checksum(self, source_ref: str) -> str | None:
        """Return the checksum verified during this adapter instance's fetch."""
        return self._observed_checksums.get(source_ref)

    def normalize(self, chunk: FetchChunk, raw: bytes) -> NormalizedBatch:
        """Normalize a verified archive into canonical F0 or F0T market events."""
        archive_day = self._archive_day(chunk.source_ref)
        rows = self._read_archive(chunk.source_ref, raw)
        events: list[dict[str, JsonValue]] = []
        for row_index, row in enumerate(rows):
            if not row:
                continue
            if row_index == 0 and not row[0].strip().lstrip("-").isdigit():
                continue
            if self.config.data_kind == BinanceDataKind.KLINES:
                events.append(self._normalize_kline(row, archive_day))
            elif self.config.data_kind == BinanceDataKind.TRADES:
                events.append(self._normalize_trade(row, archive_day, aggregate=False))
            else:
                events.append(self._normalize_trade(row, archive_day, aggregate=True))
        return NormalizedBatch(tuple(events))

    def _normalize_trade(
        self, row: list[str], archive_day: date, *, aggregate: bool
    ) -> dict[str, JsonValue]:
        minimum_columns = 7 if aggregate else 6
        if self.config.product == BinanceProduct.SPOT:
            minimum_columns += 1
        if len(row) != minimum_columns:
            raise BinanceSchemaError(
                f"unexpected {self.config.data_kind.value} column count: {len(row)}"
            )
        try:
            trade_id = _canonical_uint(row[0])
            price_atoms = _decimal_atoms(row[1], self.config.price_scale, signed=True)
            qty_atoms = _decimal_atoms(row[2], self.config.qty_scale, signed=False, positive=True)
            timestamp_index = 5 if aggregate else 4
            maker_index = 6 if aggregate else 5
            timestamp = _canonical_uint(row[timestamp_index])
            buyer_is_maker = _boolean(row[maker_index])
        except (IndexError, ValueError) as error:
            raise BinanceSchemaError("malformed Binance trade row") from error
        ts_event_ns = _timestamp_to_ns(timestamp, self.config.product, archive_day)
        aggressor = "SELL" if buyer_is_maker else "BUY"
        return self._event(
            ts_event_ns=ts_event_ns,
            tie_breaker=trade_id,
            source_sequence=trade_id,
            source_event_id=trade_id,
            kind="TRADE",
            payload={
                "price_atoms": str(price_atoms),
                "qty_atoms": str(qty_atoms),
                "aggressor_side": aggressor,
                "trade_id": trade_id,
            },
        )

    def _normalize_kline(self, row: list[str], archive_day: date) -> dict[str, JsonValue]:
        if len(row) != 12:
            raise BinanceSchemaError(f"unexpected kline column count: {len(row)}")
        assert self.config.interval is not None
        try:
            open_timestamp = _canonical_uint(row[0])
            open_atoms = _decimal_atoms(row[1], self.config.price_scale, signed=True)
            high_atoms = _decimal_atoms(row[2], self.config.price_scale, signed=True)
            low_atoms = _decimal_atoms(row[3], self.config.price_scale, signed=True)
            close_atoms = _decimal_atoms(row[4], self.config.price_scale, signed=True)
            volume_atoms = _decimal_atoms(row[5], self.config.qty_scale, signed=False)
            trade_count = _canonical_uint(row[8])
        except (IndexError, ValueError) as error:
            raise BinanceSchemaError("malformed Binance kline row") from error
        ts_event_ns = _timestamp_to_ns(open_timestamp, self.config.product, archive_day)
        return self._event(
            ts_event_ns=ts_event_ns,
            tie_breaker="0",
            source_sequence=None,
            source_event_id=f"{self.config.interval}:{open_timestamp}",
            kind="BAR",
            payload={
                "interval": self.config.interval,
                "open_atoms": str(open_atoms),
                "high_atoms": str(high_atoms),
                "low_atoms": str(low_atoms),
                "close_atoms": str(close_atoms),
                "base_volume_atoms": str(volume_atoms),
                "trade_count": trade_count,
                "complete": True,
            },
        )

    def _event(
        self,
        *,
        ts_event_ns: int,
        tie_breaker: str,
        source_sequence: str | None,
        source_event_id: str,
        kind: str,
        payload: dict[str, JsonValue],
    ) -> dict[str, JsonValue]:
        event: dict[str, JsonValue] = {
            "schema_version": "1.0.0",
            "dataset_id": self.config.dataset_id,
            "instrument_id": self.config.instrument_id,
            "venue_id": self.config.venue_id,
            "ts_event_ns": str(ts_event_ns),
            "canonical_tie_breaker": tie_breaker,
            "source_event_id": source_event_id,
            "kind": kind,
            "payload": payload,
            "quality_flags": [],
        }
        if source_sequence is not None:
            event["source_sequence"] = source_sequence
        return event

    def _read_archive(self, source_ref: str, raw: bytes) -> list[list[str]]:
        try:
            with zipfile.ZipFile(io.BytesIO(raw)) as archive:
                infos = archive.infolist()
                if len(infos) != 1:
                    raise BinanceIntegrityError("archive must contain exactly one CSV file")
                info = infos[0]
                expected_csv = PurePosixPath(source_ref).name.removesuffix(".zip") + ".csv"
                if info.is_dir() or PurePosixPath(info.filename).name != info.filename:
                    raise BinanceIntegrityError("archive member path is unsafe")
                if info.filename != expected_csv:
                    raise BinanceIntegrityError("archive member name does not match source object")
                if info.file_size > self.config.max_uncompressed_bytes:
                    raise BinanceIntegrityError("archive member exceeds uncompressed byte ceiling")
                with archive.open(info) as member:
                    payload = member.read(self.config.max_uncompressed_bytes + 1)
        except zipfile.BadZipFile as error:
            raise BinanceIntegrityError("invalid Binance ZIP archive") from error
        if len(payload) > self.config.max_uncompressed_bytes:
            raise BinanceIntegrityError("archive member exceeds uncompressed byte ceiling")
        try:
            text = payload.decode("utf-8", errors="strict")
        except UnicodeDecodeError as error:
            raise BinanceSchemaError("archive CSV must be UTF-8") from error
        return list(csv.reader(io.StringIO(text, newline="")))

    def _parse_checksum(self, source_ref: str, document: bytes) -> str:
        try:
            text = document.decode("ascii", errors="strict").strip()
        except UnicodeDecodeError as error:
            raise BinanceIntegrityError("CHECKSUM must be ASCII") from error
        parts = text.split()
        if len(parts) != 2:
            raise BinanceIntegrityError("malformed CHECKSUM document")
        digest = parts[0].lower()
        filename = parts[1].removeprefix("*")
        if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest):
            raise BinanceIntegrityError("CHECKSUM does not contain a SHA-256 digest")
        if filename != PurePosixPath(source_ref).name:
            raise BinanceIntegrityError("CHECKSUM filename does not match archive")
        return digest

    def _validate_source_ref(self, source_ref: str) -> None:
        if not source_ref.startswith(f"{BASE_URL}/data/") or not source_ref.endswith(".zip"):
            raise BinanceRequestError("chunk source is outside official Binance archive")
        self._archive_day(source_ref)

    def _archive_day(self, source_ref: str) -> date:
        filename = PurePosixPath(source_ref).name
        if not filename.endswith(".zip"):
            raise BinanceRequestError("source object must be a ZIP archive")
        date_text = filename.removesuffix(".zip")[-10:]
        try:
            return date.fromisoformat(date_text)
        except ValueError as error:
            raise BinanceRequestError("archive filename does not end in an ISO date") from error


def _timestamp_to_ns(raw: str, product: BinanceProduct, archive_day: date) -> int:
    value = int(raw, 10)
    multiplier = (
        1_000
        if product == BinanceProduct.SPOT and archive_day >= SPOT_MICROSECOND_START
        else 1_000_000
    )
    result = value * multiplier
    if result > 2**63 - 1:
        raise BinanceSchemaError("timestamp exceeds canonical signed 64-bit range")
    return result


def _canonical_uint(value: str) -> str:
    text = value.strip()
    if not text or not text.isascii() or not text.isdigit():
        raise ValueError("expected unsigned decimal integer")
    if len(text) > 1 and text.startswith("0"):
        raise ValueError("non-canonical unsigned integer")
    if int(text, 10) > 2**64 - 1:
        raise ValueError("unsigned integer exceeds 64-bit range")
    return text


def _decimal_atoms(value: str, scale: int, *, signed: bool, positive: bool = False) -> int:
    try:
        decimal = Decimal(value.strip())
    except InvalidOperation as error:
        raise ValueError("invalid decimal") from error
    if not decimal.is_finite():
        raise ValueError("non-finite decimal")
    scaled = decimal.scaleb(scale)
    integral = scaled.to_integral_value()
    if scaled != integral:
        raise ValueError("decimal cannot be represented at declared instrument scale")
    atoms = int(integral)
    if positive and atoms <= 0:
        raise ValueError("quantity must be positive")
    if not signed and atoms < 0:
        raise ValueError("unsigned atoms cannot be negative")
    lower = -(2**63) if signed else 0
    upper = 2**63 - 1 if signed else 2**64 - 1
    if atoms < lower or atoms > upper:
        raise ValueError("scaled atoms exceed canonical integer range")
    return atoms


def _boolean(value: str) -> bool:
    normalized = value.strip().lower()
    if normalized == "true":
        return True
    if normalized == "false":
        return False
    raise ValueError("expected true/false")


__all__ = [
    "ArchiveRevisionDetected",
    "BinanceDataKind",
    "BinanceIntegrityError",
    "BinanceProduct",
    "BinancePublicAdapter",
    "BinancePublicConfig",
    "BinancePublicError",
    "BinanceRequestError",
    "BinanceSchemaError",
    "HttpTransport",
    "UrlLibTransport",
]
