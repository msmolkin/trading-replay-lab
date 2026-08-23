"""Deterministic replay calendars and interval boundary calculation."""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import UTC, date, datetime, time, timedelta
from zoneinfo import ZoneInfo

NANOSECONDS_PER_SECOND = 1_000_000_000
NANOSECONDS_PER_DAY = 86_400 * NANOSECONDS_PER_SECOND
_EPOCH = datetime(1970, 1, 1, tzinfo=UTC)


@dataclass(frozen=True, slots=True)
class Interval:
    """Supported canonical aggregation interval."""

    name: str
    fixed_ns: int | None = None
    calendar_unit: str | None = None


def parse_interval(value: str) -> Interval:
    """Parse fixed minute/hour intervals plus day/week/month calendar intervals."""
    if value == "1d":
        return Interval(value, calendar_unit="day")
    if value == "1w":
        return Interval(value, calendar_unit="week")
    if value == "1mo":
        return Interval(value, calendar_unit="month")
    if len(value) < 2 or not value[:-1].isdigit():
        raise ValueError(f"unsupported interval: {value}")
    count = int(value[:-1])
    if count <= 0:
        raise ValueError("interval count must be positive")
    unit = value[-1]
    multiplier = {"s": 1, "m": 60, "h": 3_600}.get(unit)
    if multiplier is None:
        raise ValueError(f"unsupported interval: {value}")
    return Interval(value, fixed_ns=count * multiplier * NANOSECONDS_PER_SECOND)


def datetime_to_ns(value: datetime) -> int:
    """Convert an aware datetime to epoch nanoseconds without float timestamps."""
    if value.tzinfo is None:
        raise ValueError("datetime must be timezone-aware")
    delta = value.astimezone(UTC) - _EPOCH
    return (
        delta.days * NANOSECONDS_PER_DAY
        + delta.seconds * NANOSECONDS_PER_SECOND
        + delta.microseconds * 1_000
    )


def ns_to_datetime(value: int) -> datetime:
    """Convert epoch nanoseconds to UTC datetime at microsecond calendar precision."""
    seconds, nanoseconds = divmod(value, NANOSECONDS_PER_SECOND)
    return datetime.fromtimestamp(seconds, tz=UTC).replace(microsecond=nanoseconds // 1_000)


@dataclass(frozen=True, slots=True)
class SessionWindow:
    """One local trading session represented as half-open UTC nanoseconds."""

    session_date: date
    start_ns: int
    end_ns: int

    def contains(self, ts_ns: int) -> bool:
        return self.start_ns <= ts_ns < self.end_ns


@dataclass(frozen=True, slots=True)
class SessionCalendar:
    """Weekday/holiday session calendar with optional overnight sessions and halts."""

    zone_key: str
    open_time: time
    close_time: time
    weekdays: frozenset[int] = field(default_factory=lambda: frozenset(range(5)))
    holidays: frozenset[date] = field(default_factory=frozenset)
    halts: tuple[tuple[int, int], ...] = ()

    @property
    def zone(self) -> ZoneInfo:
        return ZoneInfo(self.zone_key)

    def session_for_date(self, session_date: date) -> SessionWindow | None:
        """Return the declared session originating on a local calendar date."""
        if session_date.weekday() not in self.weekdays or session_date in self.holidays:
            return None
        close_date = session_date
        if self.close_time <= self.open_time:
            close_date += timedelta(days=1)
        start = datetime.combine(session_date, self.open_time, self.zone)
        end = datetime.combine(close_date, self.close_time, self.zone)
        start_ns = datetime_to_ns(start)
        end_ns = datetime_to_ns(end)
        if end_ns <= start_ns:
            raise ValueError("session close must follow open")
        return SessionWindow(session_date, start_ns, end_ns)

    def session_for_ns(self, ts_ns: int) -> SessionWindow | None:
        """Find the local session containing a UTC timestamp."""
        local_date = ns_to_datetime(ts_ns).astimezone(self.zone).date()
        for candidate in (local_date, local_date - timedelta(days=1)):
            window = self.session_for_date(candidate)
            if window is not None and window.contains(ts_ns):
                return window
        return None

    def is_halted(self, ts_ns: int) -> bool:
        return any(start <= ts_ns < end for start, end in self.halts)

    def _period_sessions(self, start_date: date, end_date: date) -> list[SessionWindow]:
        sessions: list[SessionWindow] = []
        cursor = start_date
        while cursor < end_date:
            window = self.session_for_date(cursor)
            if window is not None:
                sessions.append(window)
            cursor += timedelta(days=1)
        return sessions

    def bucket_bounds(self, ts_ns: int, interval: Interval) -> tuple[int, int]:
        """Return half-open UTC bucket bounds for an in-session event."""
        session = self.session_for_ns(ts_ns)
        if session is None:
            raise ValueError("timestamp is outside a trading session")
        if interval.fixed_ns is not None:
            elapsed = ts_ns - session.start_ns
            start = session.start_ns + (elapsed // interval.fixed_ns) * interval.fixed_ns
            return start, min(start + interval.fixed_ns, session.end_ns)
        if interval.calendar_unit == "day":
            return session.start_ns, session.end_ns
        if interval.calendar_unit == "week":
            monday = session.session_date - timedelta(days=session.session_date.weekday())
            sessions = self._period_sessions(monday, monday + timedelta(days=7))
        elif interval.calendar_unit == "month":
            month_start = session.session_date.replace(day=1)
            next_month = (
                month_start.replace(year=month_start.year + 1, month=1)
                if month_start.month == 12
                else month_start.replace(month=month_start.month + 1)
            )
            sessions = self._period_sessions(month_start, next_month)
        else:
            raise ValueError(f"unsupported interval: {interval.name}")
        if not sessions:
            raise ValueError("calendar period contains no sessions")
        return sessions[0].start_ns, sessions[-1].end_ns


@dataclass(frozen=True, slots=True)
class CryptoUtcCalendar:
    """Continuous UTC crypto calendar."""

    def bucket_bounds(self, ts_ns: int, interval: Interval) -> tuple[int, int]:
        if interval.fixed_ns is not None:
            start = (ts_ns // interval.fixed_ns) * interval.fixed_ns
            return start, start + interval.fixed_ns
        current = ns_to_datetime(ts_ns)
        if interval.calendar_unit == "day":
            start_dt = datetime(current.year, current.month, current.day, tzinfo=UTC)
            end_dt = start_dt + timedelta(days=1)
        elif interval.calendar_unit == "week":
            start_date = current.date() - timedelta(days=current.weekday())
            start_dt = datetime.combine(start_date, time(0), UTC)
            end_dt = start_dt + timedelta(days=7)
        elif interval.calendar_unit == "month":
            start_dt = datetime(current.year, current.month, 1, tzinfo=UTC)
            end_dt = (
                datetime(current.year + 1, 1, 1, tzinfo=UTC)
                if current.month == 12
                else datetime(current.year, current.month + 1, 1, tzinfo=UTC)
            )
        else:
            raise ValueError(f"unsupported interval: {interval.name}")
        return datetime_to_ns(start_dt), datetime_to_ns(end_dt)


__all__ = [
    "CryptoUtcCalendar",
    "Interval",
    "SessionCalendar",
    "SessionWindow",
    "datetime_to_ns",
    "ns_to_datetime",
    "parse_interval",
]
