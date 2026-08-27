"""Tardis dataset declarations, entitlement checks, and interval capabilities."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from datetime import date
from enum import StrEnum
from typing import Protocol, cast


class TardisDataType(StrEnum):
    """Normalized downloadable CSV datasets used by replay ingestion."""

    TRADES = "trades"
    QUOTES = "quotes"
    BOOK_TICKER = "book_ticker"
    INCREMENTAL_BOOK_L2 = "incremental_book_L2"
    BOOK_SNAPSHOT_25 = "book_snapshot_25"
    BOOK_SNAPSHOT_5 = "book_snapshot_5"
    DERIVATIVE_TICKER = "derivative_ticker"
    LIQUIDATIONS = "liquidations"


class TardisError(ValueError):
    """Base class for deterministic Tardis adapter failures."""


class TardisRequestError(TardisError):
    """Request or entitlement cannot be represented safely."""


class TardisSchemaError(TardisError):
    """Downloaded normalized CSV violates the declared provider schema."""


class TardisIntegrityError(TardisError):
    """Downloaded object violates a configured resource or compression invariant."""


class ApiKeyProvider(Protocol):
    """Ephemeral secret boundary; credentials are never part of adapter state."""

    def __call__(self) -> str | None:
        """Return the current Tardis API key, or ``None`` for public sample access."""
        ...


@dataclass(frozen=True, slots=True)
class TardisCoverage:
    """Point-in-time dataset coverage for one exchange symbol."""

    exchange: str
    symbol: str
    data_types: frozenset[TardisDataType]
    available_since: date
    available_to: date
    exported_until: date | None = None

    def __post_init__(self) -> None:
        if not self.exchange or not self.symbol:
            raise ValueError("exchange and symbol are required")
        if self.available_to < self.available_since:
            raise ValueError("coverage end precedes coverage start")
        if not self.data_types:
            raise ValueError("coverage must declare at least one data type")

    def supports(self, data_type: TardisDataType, day: date) -> bool:
        """Return whether one UTC dataset day is declared available."""
        if data_type not in self.data_types:
            return False
        effective_to = self.available_to
        if self.exported_until is not None:
            effective_to = min(effective_to, self.exported_until)
        return self.available_since <= day <= effective_to

    @classmethod
    def from_exchange_details(
        cls,
        document: Mapping[str, object],
        *,
        exchange: str,
        symbol: str,
    ) -> TardisCoverage:
        """Parse one ``/v1/exchanges/:exchange`` response fail closed."""
        datasets = _mapping(document.get("datasets"), "datasets")
        symbols = _sequence(datasets.get("symbols"), "datasets.symbols")
        target: Mapping[str, object] | None = None
        for candidate in symbols:
            row = _mapping(candidate, "datasets.symbols[]")
            if row.get("id") == symbol:
                target = row
                break
        if target is None:
            raise TardisRequestError("symbol is not present in Tardis dataset coverage")

        raw_types = _sequence(target.get("dataTypes"), "datasets.symbols[].dataTypes")
        supported: set[TardisDataType] = set()
        for raw in raw_types:
            if not isinstance(raw, str):
                raise TardisRequestError("Tardis dataTypes entries must be strings")
            try:
                supported.add(TardisDataType(raw))
            except ValueError:
                continue
        if not supported:
            raise TardisRequestError("symbol has no replay-supported Tardis datasets")

        available_since = _date_field(target, "availableSince")
        available_to = _date_field(target, "availableTo")
        exported = datasets.get("exportedUntil")
        exported_until = None if exported in (None, "") else _parse_date(exported, "datasets.exportedUntil")
        return cls(
            exchange=exchange,
            symbol=symbol,
            data_types=frozenset(supported),
            available_since=available_since,
            available_to=available_to,
            exported_until=exported_until,
        )


@dataclass(frozen=True, slots=True)
class TardisEntitlement:
    """Explicit local policy describing which provider objects may be requested."""

    coverage: TardisCoverage
    allowed_data_types: frozenset[TardisDataType]
    sample_only: bool = True

    def permits(self, data_type: TardisDataType, day: date) -> bool:
        """Return whether local policy and provider coverage both permit a dataset day."""
        if data_type not in self.allowed_data_types or not self.coverage.supports(data_type, day):
            return False
        return not self.sample_only or day.day == 1


@dataclass(frozen=True, slots=True)
class TardisCapabilities:
    """Conservative fidelity facts for one covered interval."""

    has_trades: bool
    has_bbo: bool
    has_l2_snapshots: bool
    has_l2_deltas: bool
    has_funding: bool
    has_liquidations: bool
    execution_tier: str | None


def capabilities_for_interval(
    entitlement: TardisEntitlement,
    *,
    start_day: date,
    end_day: date,
) -> TardisCapabilities:
    """Compute capabilities only when each required dataset covers every day in the interval."""
    if end_day < start_day:
        raise ValueError("capability interval end precedes start")

    def complete(data_type: TardisDataType) -> bool:
        current = start_day
        while current <= end_day:
            if not entitlement.permits(data_type, current):
                return False
            current = date.fromordinal(current.toordinal() + 1)
        return True

    trades = complete(TardisDataType.TRADES)
    bbo = complete(TardisDataType.QUOTES) or complete(TardisDataType.BOOK_TICKER)
    incremental_l2 = complete(TardisDataType.INCREMENTAL_BOOK_L2)
    snapshots = incremental_l2 or complete(TardisDataType.BOOK_SNAPSHOT_25) or complete(
        TardisDataType.BOOK_SNAPSHOT_5
    )
    funding = complete(TardisDataType.DERIVATIVE_TICKER)
    liquidations = complete(TardisDataType.LIQUIDATIONS)
    if incremental_l2:
        tier = "F2"
    elif trades and bbo:
        tier = "F1"
    elif trades:
        tier = "F0T"
    else:
        tier = None
    return TardisCapabilities(
        has_trades=trades,
        has_bbo=bbo,
        has_l2_snapshots=snapshots,
        has_l2_deltas=incremental_l2,
        has_funding=funding,
        has_liquidations=liquidations,
        execution_tier=tier,
    )


def _mapping(value: object, name: str) -> Mapping[str, object]:
    if not isinstance(value, Mapping):
        raise TardisRequestError(f"{name} must be an object")
    if any(not isinstance(key, str) for key in value):
        raise TardisRequestError(f"{name} keys must be strings")
    return cast(Mapping[str, object], value)


def _sequence(value: object, name: str) -> Sequence[object]:
    if not isinstance(value, Sequence) or isinstance(value, str | bytes):
        raise TardisRequestError(f"{name} must be an array")
    return cast(Sequence[object], value)


def _date_field(fields: Mapping[str, object], name: str) -> date:
    return _parse_date(fields.get(name), name)


def _parse_date(value: object, name: str) -> date:
    if not isinstance(value, str):
        raise TardisRequestError(f"{name} must be an ISO date")
    try:
        return date.fromisoformat(value[:10])
    except ValueError as error:
        raise TardisRequestError(f"{name} must be an ISO date") from error
