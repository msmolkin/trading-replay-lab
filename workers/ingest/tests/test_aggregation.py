from __future__ import annotations

from datetime import UTC, datetime, time

from trading_replay_ingest.aggregate import KnownGap, Trade, aggregate_trades
from trading_replay_ingest.calendars import (
    CryptoUtcCalendar,
    SessionCalendar,
    datetime_to_ns,
    ns_to_datetime,
    parse_interval,
)


def ns(value: datetime) -> int:
    return datetime_to_ns(value)


def test_crypto_integer_ohlcv_and_empty_periods_are_not_fabricated() -> None:
    minute = 60_000_000_000
    trades = [
        Trade(5, 100, 2),
        Trade(10, 105, 3),
        Trade(20, 95, 4),
        Trade(2 * minute + 1, 110, 1),
    ]
    bars = aggregate_trades(
        trades,
        interval_name="1m",
        calendar=CryptoUtcCalendar(),
        coverage_end_ns=3 * minute,
    )
    assert len(bars) == 2
    assert (bars[0].open_atoms, bars[0].high_atoms, bars[0].low_atoms, bars[0].close_atoms) == (
        100,
        105,
        95,
        95,
    )
    assert bars[0].base_volume_atoms == 9
    assert bars[0].trade_count == 3
    assert bars[1].start_ns == 2 * minute


def test_gap_and_partial_frontier_are_explicit() -> None:
    minute = 60_000_000_000
    bars = aggregate_trades(
        [Trade(10, 100, 1), Trade(minute + 10, 101, 1)],
        interval_name="1m",
        calendar=CryptoUtcCalendar(),
        coverage_end_ns=minute + 30,
        known_gaps=(KnownGap(20, 40),),
    )
    assert bars[0].complete is False
    assert bars[0].quality_flags == ("KNOWN_GAP",)
    assert bars[1].complete is False
    assert bars[1].quality_flags == ("INCOMPLETE_COVERAGE",)


def test_new_york_session_open_moves_across_dst_without_wall_clock_guessing() -> None:
    calendar = SessionCalendar("America/New_York", time(9, 30), time(16, 0))
    before = calendar.session_for_date(datetime(2026, 3, 6).date())
    after = calendar.session_for_date(datetime(2026, 3, 9).date())
    assert before is not None and after is not None
    assert ns_to_datetime(before.start_ns).hour == 14
    assert ns_to_datetime(before.start_ns).minute == 30
    assert ns_to_datetime(after.start_ns).hour == 13
    assert ns_to_datetime(after.start_ns).minute == 30


def test_overnight_future_session_resolves_to_originating_date() -> None:
    calendar = SessionCalendar("UTC", time(18, 0), time(17, 0))
    timestamp = ns(datetime(2026, 3, 10, 2, 0, tzinfo=UTC))
    session = calendar.session_for_ns(timestamp)
    assert session is not None
    assert session.session_date.isoformat() == "2026-03-09"


def test_halt_intersection_flags_bar_without_changing_prices() -> None:
    session_start = ns(datetime(2026, 3, 9, 13, 30, tzinfo=UTC))
    halt = (session_start + 10_000_000_000, session_start + 20_000_000_000)
    calendar = SessionCalendar(
        "America/New_York",
        time(9, 30),
        time(16, 0),
        halts=(halt,),
    )
    bars = aggregate_trades(
        [Trade(session_start + 1, 100, 1)],
        interval_name="1m",
        calendar=calendar,
        coverage_end_ns=session_start + 120_000_000_000,
    )
    assert bars[0].quality_flags == ("SESSION_HALT",)
    assert bars[0].open_atoms == bars[0].close_atoms == 100


def test_week_and_month_crypto_boundaries_are_calendar_aligned() -> None:
    calendar = CryptoUtcCalendar()
    timestamp = ns(datetime(2026, 8, 23, 12, 0, tzinfo=UTC))
    week_start, week_end = calendar.bucket_bounds(timestamp, parse_interval("1w"))
    month_start, month_end = calendar.bucket_bounds(timestamp, parse_interval("1mo"))
    assert ns_to_datetime(week_start).date().isoformat() == "2026-08-17"
    assert ns_to_datetime(week_end).date().isoformat() == "2026-08-24"
    assert ns_to_datetime(month_start).date().isoformat() == "2026-08-01"
    assert ns_to_datetime(month_end).date().isoformat() == "2026-09-01"


def test_out_of_order_trade_stream_fails_closed() -> None:
    try:
        aggregate_trades(
            [Trade(20, 100, 1), Trade(10, 101, 1)],
            interval_name="1m",
            calendar=CryptoUtcCalendar(),
            coverage_end_ns=100,
        )
    except ValueError as error:
        assert "non-decreasing" in str(error)
    else:
        raise AssertionError("out-of-order trades must be rejected")
