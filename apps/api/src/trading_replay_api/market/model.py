"""Visibility-safe market data types and source/session boundaries."""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum
from typing import Protocol

from trading_replay_api.sessions import SessionRecord

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1
U64_MAX = 2**64 - 1


class MarketErrorCode(StrEnum):
    """Stable market API failure codes."""

    SESSION_UNAVAILABLE = "SESSION_UNAVAILABLE"
    SESSION_NOT_COMMITTED = "SESSION_NOT_COMMITTED"
    INVALID_QUERY = "INVALID_QUERY"
    UNSUPPORTED_INTERVAL = "UNSUPPORTED_INTERVAL"
    SOURCE_ORDER = "SOURCE_ORDER"


class MarketServiceError(RuntimeError):
    """Visibility market-data error carrying a stable code."""

    def __init__(self, code: MarketErrorCode, message: str) -> None:
        super().__init__(message)
        self.code = code


class MarketChannel(StrEnum):
    """Supported visible market data channels."""

    TAPE = "TAPE"
    BBO = "BBO"
    DEPTH = "DEPTH"


@dataclass(frozen=True, slots=True)
class Trade:
    """Canonical source trade before visibility projection."""

    source_event_id: str
    source_sequence: int
    ts_ns: int
    price_atoms: int
    quantity_atoms: int

    def __post_init__(self) -> None:
        _identity(self.source_event_id, "source_event_id")
        _u64(self.source_sequence, "source_sequence")
        _i64(self.ts_ns, "ts_ns")
        _positive_i64(self.price_atoms, "price_atoms")
        _positive_u64(self.quantity_atoms, "quantity_atoms")


@dataclass(frozen=True, slots=True)
class Bbo:
    """Canonical best-bid/offer source event."""

    source_event_id: str
    source_sequence: int
    ts_ns: int
    bid_price_atoms: int
    bid_quantity_atoms: int
    ask_price_atoms: int
    ask_quantity_atoms: int

    def __post_init__(self) -> None:
        _identity(self.source_event_id, "source_event_id")
        _u64(self.source_sequence, "source_sequence")
        _i64(self.ts_ns, "ts_ns")
        _positive_i64(self.bid_price_atoms, "bid_price_atoms")
        _positive_i64(self.ask_price_atoms, "ask_price_atoms")
        _positive_u64(self.bid_quantity_atoms, "bid_quantity_atoms")
        _positive_u64(self.ask_quantity_atoms, "ask_quantity_atoms")
        if self.bid_price_atoms >= self.ask_price_atoms:
            raise ValueError("BBO must have bid below ask")


@dataclass(frozen=True, slots=True)
class DepthLevel:
    """One exact visible L2 price level."""

    price_atoms: int
    quantity_atoms: int

    def __post_init__(self) -> None:
        _positive_i64(self.price_atoms, "price_atoms")
        _positive_u64(self.quantity_atoms, "quantity_atoms")


@dataclass(frozen=True, slots=True)
class DepthSnapshot:
    """Canonical source L2 snapshot before visibility projection."""

    source_event_id: str
    source_sequence: int
    ts_ns: int
    bids: tuple[DepthLevel, ...]
    asks: tuple[DepthLevel, ...]

    def __post_init__(self) -> None:
        _identity(self.source_event_id, "source_event_id")
        _u64(self.source_sequence, "source_sequence")
        _i64(self.ts_ns, "ts_ns")
        if not self.bids or not self.asks:
            raise ValueError("depth snapshot requires both sides")
        if max(level.price_atoms for level in self.bids) >= min(
            level.price_atoms for level in self.asks
        ):
            raise ValueError("depth snapshot cannot be crossed")


class MarketDataSource(Protocol):
    """Canonical source boundary; implementations may read catalog partitions."""

    def trades(
        self,
        *,
        manifest_hash: str,
        instrument_id: str,
        start_ns: int,
        through_ns: int,
        after_source_sequence: int | None,
        limit: int,
    ) -> tuple[Trade, ...]:
        """Return source trades ordered by sequence/time."""
        ...

    def bbo(
        self,
        *,
        manifest_hash: str,
        instrument_id: str,
        start_ns: int,
        through_ns: int,
        after_source_sequence: int | None,
        limit: int,
    ) -> tuple[Bbo, ...]:
        """Return source quotes ordered by sequence/time."""
        ...

    def depth(
        self,
        *,
        manifest_hash: str,
        instrument_id: str,
        start_ns: int,
        through_ns: int,
        after_source_sequence: int | None,
        limit: int,
    ) -> tuple[DepthSnapshot, ...]:
        """Return source depth snapshots ordered by sequence/time."""
        ...

    def latest_bbo(
        self,
        *,
        manifest_hash: str,
        instrument_id: str,
        start_ns: int,
        through_ns: int,
    ) -> Bbo | None:
        """Return the latest quote no later than through_ns."""
        ...


class SessionReader(Protocol):
    """Principal-scoped persisted session state boundary."""

    def get_session(self, *, session_id: str, principal_id: str) -> SessionRecord:
        """Return one owned session or raise without leaking another principal."""
        ...


def _identity(value: str, name: str) -> None:
    if not value or any(character in value for character in "\x00\r\n"):
        raise ValueError(f"invalid {name}")


def _i64(value: int, name: str) -> None:
    if isinstance(value, bool) or value < I64_MIN or value > I64_MAX:
        raise ValueError(f"{name} must fit signed 64-bit integer")


def _u64(value: int, name: str) -> None:
    if isinstance(value, bool) or value < 0 or value > U64_MAX:
        raise ValueError(f"{name} must fit unsigned 64-bit integer")


def _positive_i64(value: int, name: str) -> None:
    _i64(value, name)
    if value <= 0:
        raise ValueError(f"{name} must be positive")


def _positive_u64(value: int, name: str) -> None:
    _u64(value, name)
    if value == 0:
        raise ValueError(f"{name} must be positive")


__all__ = [
    "Bbo",
    "DepthLevel",
    "DepthSnapshot",
    "MarketChannel",
    "MarketDataSource",
    "MarketErrorCode",
    "MarketServiceError",
    "SessionReader",
    "Trade",
]
