"""Deterministic integer trade-to-bar aggregation."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol, Sequence

from trading_replay_ingest.calendars import Interval, SessionCalendar, parse_interval


class BucketCalendar(Protocol):
    def bucket_bounds(self, ts_ns: int, interval: Interval) -> tuple[int, int]: ...


@dataclass(frozen=True, slots=True)
class Trade:
    """Minimal exact trade input for bar aggregation."""

    ts_event_ns: int
    price_atoms: int
    qty_atoms: int

    def __post_init__(self) -> None:
        if self.price_atoms <= 0:
            raise ValueError("trade price must be positive")
        if self.qty_atoms <= 0:
            raise ValueError("trade quantity must be positive")


@dataclass(frozen=True, slots=True)
class KnownGap:
    """Half-open source-data gap."""

    start_ns: int
    end_ns: int

    def __post_init__(self) -> None:
        if self.end_ns <= self.start_ns:
            raise ValueError("gap end must follow start")

    def overlaps(self, start_ns: int, end_ns: int) -> bool:
        return self.start_ns < end_ns and start_ns < self.end_ns


@dataclass(frozen=True, slots=True)
class Bar:
    """Exact OHLCV bar with explicit completeness/quality flags."""

    interval: str
    start_ns: int
    end_ns: int
    open_atoms: int
    high_atoms: int
    low_atoms: int
    close_atoms: int
    base_volume_atoms: int
    trade_count: int
    complete: bool
    quality_flags: tuple[str, ...]


@dataclass(slots=True)
class _MutableBar:
    start_ns: int
    end_ns: int
    open_atoms: int
    high_atoms: int
    low_atoms: int
    close_atoms: int
    base_volume_atoms: int
    trade_count: int

    @classmethod
    def from_trade(cls, start_ns: int, end_ns: int, trade: Trade) -> _MutableBar:
        return cls(
            start_ns=start_ns,
            end_ns=end_ns,
            open_atoms=trade.price_atoms,
            high_atoms=trade.price_atoms,
            low_atoms=trade.price_atoms,
            close_atoms=trade.price_atoms,
            base_volume_atoms=trade.qty_atoms,
            trade_count=1,
        )

    def add(self, trade: Trade) -> None:
        self.high_atoms = max(self.high_atoms, trade.price_atoms)
        self.low_atoms = min(self.low_atoms, trade.price_atoms)
        self.close_atoms = trade.price_atoms
        self.base_volume_atoms += trade.qty_atoms
        self.trade_count += 1


def _halt_overlaps(calendar: BucketCalendar, start_ns: int, end_ns: int) -> bool:
    if not isinstance(calendar, SessionCalendar):
        return False
    return any(halt_start < end_ns and start_ns < halt_end for halt_start, halt_end in calendar.halts)


def aggregate_trades(
    trades: Sequence[Trade],
    *,
    interval_name: str,
    calendar: BucketCalendar,
    coverage_end_ns: int,
    known_gaps: Sequence[KnownGap] = (),
) -> tuple[Bar, ...]:
    """Aggregate ordered trades without float math or synthetic empty candles.

    Empty periods remain absent rather than inventing prices. A bar is incomplete when
    its bucket extends past the known coverage frontier or intersects a declared source gap.
    """
    interval = parse_interval(interval_name)
    mutable: list[_MutableBar] = []
    previous_ts: int | None = None
    current: _MutableBar | None = None

    for trade in trades:
        if previous_ts is not None and trade.ts_event_ns < previous_ts:
            raise ValueError("trades must be non-decreasing by event timestamp")
        previous_ts = trade.ts_event_ns
        start_ns, end_ns = calendar.bucket_bounds(trade.ts_event_ns, interval)
        if current is None or (current.start_ns, current.end_ns) != (start_ns, end_ns):
            current = _MutableBar.from_trade(start_ns, end_ns, trade)
            mutable.append(current)
        else:
            current.add(trade)

    bars: list[Bar] = []
    for source in mutable:
        flags: list[str] = []
        if source.end_ns > coverage_end_ns:
            flags.append("INCOMPLETE_COVERAGE")
        if any(gap.overlaps(source.start_ns, source.end_ns) for gap in known_gaps):
            flags.append("KNOWN_GAP")
        if _halt_overlaps(calendar, source.start_ns, source.end_ns):
            flags.append("SESSION_HALT")
        bars.append(
            Bar(
                interval=interval.name,
                start_ns=source.start_ns,
                end_ns=source.end_ns,
                open_atoms=source.open_atoms,
                high_atoms=source.high_atoms,
                low_atoms=source.low_atoms,
                close_atoms=source.close_atoms,
                base_volume_atoms=source.base_volume_atoms,
                trade_count=source.trade_count,
                complete=not flags,
                quality_flags=tuple(flags),
            )
        )
    return tuple(bars)


__all__ = ["Bar", "KnownGap", "Trade", "aggregate_trades"]
