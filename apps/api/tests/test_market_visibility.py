from __future__ import annotations

from dataclasses import replace

import pytest
from fastapi.routing import APIRoute

from trading_replay_api.auth import LocalAuthenticator
from trading_replay_api.catalog import DataCapability, ExecutionTier, RedistributionClass
from trading_replay_api.market import (
    Bbo,
    DepthLevel,
    DepthSnapshot,
    MarketErrorCode,
    MarketService,
    MarketServiceError,
    Trade,
    build_market_router,
)
from trading_replay_api.sessions import CommittedSetup, SessionRecord, SessionStatus, VisibilityMode

PLAY_START = 1_000_000_000_000
SECOND = 1_000_000_000
MANIFEST = "a" * 64


def setup(mode: VisibilityMode = VisibilityMode.RELATIVE) -> CommittedSetup:
    return CommittedSetup(
        instrument_id="SYNTH",
        manifest_hash=MANIFEST,
        eligibility_hash="b" * 64,
        play_start_ns=PLAY_START,
        warmup_ns=10 * SECOND,
        duration_ns=20 * SECOND,
        execution_tier=ExecutionTier.F2,
        required_capabilities=frozenset(
            {
                DataCapability.TRADES,
                DataCapability.L2_SNAPSHOTS,
                DataCapability.L2_DELTAS,
            }
        ),
        allowed_redistribution=frozenset({RedistributionClass.REDISTRIBUTABLE}),
        allow_degraded=False,
        visibility_mode=mode,
        ruleset_id="rules-1",
        ruleset_version="1",
        ruleset_hash="c" * 64,
    )


def session(
    *,
    session_id: str = "session-1",
    principal_id: str = "principal-1",
    mode: VisibilityMode = VisibilityMode.RELATIVE,
    frontier_offset: int = 2 * SECOND,
) -> SessionRecord:
    return SessionRecord(
        session_id=session_id,
        principal_id=principal_id,
        status=SessionStatus.RUNNING,
        version=5,
        created_at_ns=0,
        setup=setup(mode),
        logical_time_ns=PLAY_START + frontier_offset,
    )


class FakeSessions:
    def __init__(self, records: tuple[SessionRecord, ...]) -> None:
        self.records = {record.session_id: record for record in records}

    def get_session(self, *, session_id: str, principal_id: str) -> SessionRecord:
        record = self.records[session_id]
        if record.principal_id != principal_id:
            from trading_replay_api.sessions import SessionErrorCode, SessionLifecycleError

            raise SessionLifecycleError(SessionErrorCode.PRINCIPAL_MISMATCH, "not found")
        return record


class FakeSource:
    def __init__(self) -> None:
        self.trade_calls = 0
        self.out_of_order = False
        self.latest: Bbo | None = None

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
        del start_ns, through_ns
        assert manifest_hash == MANIFEST
        assert instrument_id == "SYNTH"
        self.trade_calls += 1
        values: tuple[Trade, ...] = (
            Trade("trade-a", 1, PLAY_START, 100, 2),
            Trade("trade-b", 2, PLAY_START + SECOND, 110, 3),
            Trade("trade-future", 3, PLAY_START + 3 * SECOND, 999, 100),
        )
        if self.out_of_order:
            values = (values[1], values[0], values[2])
        if after_source_sequence is not None:
            values = tuple(item for item in values if item.source_sequence > after_source_sequence)
        return values[:limit]

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
        del start_ns, through_ns
        assert manifest_hash == MANIFEST
        assert instrument_id == "SYNTH"
        values: tuple[Bbo, ...] = (
            Bbo("quote-a", 4, PLAY_START + SECOND, 100, 5, 102, 7),
            Bbo("quote-future", 5, PLAY_START + 4 * SECOND, 200, 1, 202, 1),
        )
        if after_source_sequence is not None:
            values = tuple(item for item in values if item.source_sequence > after_source_sequence)
        return values[:limit]

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
        del start_ns, through_ns
        assert manifest_hash == MANIFEST
        assert instrument_id == "SYNTH"
        values: tuple[DepthSnapshot, ...] = (
            DepthSnapshot(
                "depth-a",
                6,
                PLAY_START + SECOND,
                bids=(DepthLevel(100, 5),),
                asks=(DepthLevel(102, 6),),
            ),
            DepthSnapshot(
                "depth-future",
                7,
                PLAY_START + 5 * SECOND,
                bids=(DepthLevel(200, 1),),
                asks=(DepthLevel(202, 1),),
            ),
        )
        if after_source_sequence is not None:
            values = tuple(item for item in values if item.source_sequence > after_source_sequence)
        return values[:limit]

    def latest_bbo(
        self,
        *,
        manifest_hash: str,
        instrument_id: str,
        start_ns: int,
        through_ns: int,
    ) -> Bbo | None:
        del start_ns, through_ns
        assert manifest_hash == MANIFEST
        assert instrument_id == "SYNTH"
        return self.latest


def service(
    *,
    mode: VisibilityMode = VisibilityMode.RELATIVE,
) -> tuple[MarketService, FakeSource]:
    source = FakeSource()
    market = MarketService(
        sessions=FakeSessions((session(mode=mode),)),
        source=source,
    )
    return market, source


def test_future_events_are_filtered_before_relative_projection() -> None:
    market, _ = service()
    page = market.trades(session_id="session-1", principal_id="principal-1")

    assert [item["price_atoms"] for item in page.items] == ["100", "110"]
    assert [item["offset_ns"] for item in page.items] == ["0", str(SECOND)]
    assert all("ts_ns" not in item for item in page.items)
    assert page.frontier == {"offset_ns": str(2 * SECOND)}


def test_hidden_calendar_candles_never_aggregate_future_trade() -> None:
    market, _ = service(mode=VisibilityMode.HIDDEN_CALENDAR)
    page = market.candles(
        session_id="session-1",
        principal_id="principal-1",
        interval_ns=5 * SECOND,
    )

    assert len(page.items) == 1
    candle = page.items[0]
    assert candle["open_atoms"] == "100"
    assert candle["high_atoms"] == "110"
    assert candle["low_atoms"] == "100"
    assert candle["close_atoms"] == "110"
    assert candle["base_volume_atoms"] == "5"
    assert candle["trade_count"] == "2"
    assert candle["offset_ns"] == "0"
    assert "ts_ns" not in candle


def test_absolute_mode_may_render_absolute_time() -> None:
    market, _ = service(mode=VisibilityMode.ABSOLUTE)
    page = market.bbo(session_id="session-1", principal_id="principal-1")
    assert len(page.items) == 1
    assert page.items[0]["ts_ns"] == str(PLAY_START + SECOND)
    assert "offset_ns" not in page.items[0]


def test_current_quote_rejects_future_source_record_and_hashes_public_id() -> None:
    market, source = service()
    source.latest = Bbo("future", 9, PLAY_START + 3 * SECOND, 100, 1, 102, 1)
    assert market.current_quote(session_id="session-1", principal_id="principal-1") is None

    source.latest = Bbo("visible", 8, PLAY_START + SECOND, 100, 1, 102, 1)
    quote = market.current_quote(session_id="session-1", principal_id="principal-1")
    assert quote is not None
    assert quote.event_id.startswith("evt_")
    assert quote.event_id != "visible"
    assert quote.bid_price_atoms == 100
    assert quote.ask_price_atoms == 102


def test_cache_is_scoped_by_principal_session_and_frontier() -> None:
    source = FakeSource()
    sessions = FakeSessions(
        (
            session(),
            session(session_id="session-2", principal_id="principal-2"),
        )
    )
    market = MarketService(sessions=sessions, source=source)

    first = market.trades(session_id="session-1", principal_id="principal-1")
    retry = market.trades(session_id="session-1", principal_id="principal-1")
    second = market.trades(session_id="session-2", principal_id="principal-2")

    assert first == retry
    assert source.trade_calls == 2
    assert first.items[0]["event_id"] != second.items[0]["event_id"]

    sessions.records["session-1"] = replace(
        sessions.records["session-1"],
        version=6,
        logical_time_ns=PLAY_START + 4 * SECOND,
    )
    market.trades(session_id="session-1", principal_id="principal-1")
    assert source.trade_calls == 3


def test_visible_source_order_violation_fails_closed() -> None:
    market, source = service()
    source.out_of_order = True
    with pytest.raises(MarketServiceError) as caught:
        market.trades(session_id="session-1", principal_id="principal-1")
    assert caught.value.code is MarketErrorCode.SOURCE_ORDER


def test_market_router_exposes_only_visibility_gated_reads() -> None:
    market, _ = service()
    router = build_market_router(
        service=market,
        authenticator=LocalAuthenticator(principal_id="principal-1"),
    )
    paths = {route.path for route in router.routes if isinstance(route, APIRoute)}
    assert paths == {
        "/sessions/{session_id}/market/trades",
        "/sessions/{session_id}/market/bbo",
        "/sessions/{session_id}/market/depth",
        "/sessions/{session_id}/market/candles",
    }
