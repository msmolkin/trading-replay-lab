"""Session-frontier market reads with filter-before-aggregate guarantees."""

from __future__ import annotations

import hashlib
from collections.abc import Sequence
from dataclasses import dataclass
from typing import TypeVar

from trading_replay_api.commands import VisibleQuote
from trading_replay_api.sessions import (
    CommittedSetup,
    SessionLifecycleError,
    SessionRecord,
    VisibilityMode,
)

from .model import (
    Bbo,
    DepthSnapshot,
    MarketDataSource,
    MarketErrorCode,
    MarketServiceError,
    SessionReader,
    Trade,
)

T = TypeVar("T", Trade, Bbo, DepthSnapshot)

SUPPORTED_INTERVALS_NS = frozenset(
    {
        1_000_000_000,
        5_000_000_000,
        15_000_000_000,
        60_000_000_000,
        300_000_000_000,
        900_000_000_000,
        3_600_000_000_000,
        86_400_000_000_000,
    }
)
MAX_PAGE_SIZE = 1_000


@dataclass(frozen=True, slots=True)
class MarketPage:
    """One visibility-projected page and its deterministic continuation cursor."""

    items: tuple[dict[str, object], ...]
    next_cursor: str | None
    frontier: dict[str, str]


@dataclass(frozen=True, slots=True)
class _Context:
    session: SessionRecord
    frontier_ns: int | None
    start_ns: int


class MarketService:
    """Read market data only through the persisted replay frontier."""

    def __init__(self, *, sessions: SessionReader, source: MarketDataSource) -> None:
        self.sessions = sessions
        self.source = source
        self._cache: dict[tuple[object, ...], MarketPage] = {}

    def trades(
        self,
        *,
        session_id: str,
        principal_id: str,
        start_offset_ns: int = 0,
        after_sequence: int | None = None,
        limit: int = 250,
    ) -> MarketPage:
        """Return visible trades in canonical source order."""
        context = self._context(session_id, principal_id, start_offset_ns)
        cache_key = self._cache_key("TRADES", context, principal_id, after_sequence, limit, None)
        if cache_key in self._cache:
            return self._cache[cache_key]
        if context.frontier_ns is None:
            return self._cache_empty(cache_key, context)
        setup = _setup(context)
        events = self.source.trades(
            manifest_hash=setup.manifest_hash,
            instrument_id=setup.instrument_id,
            start_ns=context.start_ns,
            through_ns=context.frontier_ns,
            after_source_sequence=after_sequence,
            limit=limit,
        )
        visible = self._visible(events, context, after_sequence, limit)
        page = MarketPage(
            items=tuple(self._project_trade(context, trade) for trade in visible),
            next_cursor=_next_cursor(visible, limit),
            frontier=self._frontier_payload(context),
        )
        self._cache[cache_key] = page
        return page

    def bbo(
        self,
        *,
        session_id: str,
        principal_id: str,
        start_offset_ns: int = 0,
        after_sequence: int | None = None,
        limit: int = 250,
    ) -> MarketPage:
        """Return visible BBO events in canonical source order."""
        context = self._context(session_id, principal_id, start_offset_ns)
        cache_key = self._cache_key("BBO", context, principal_id, after_sequence, limit, None)
        if cache_key in self._cache:
            return self._cache[cache_key]
        if context.frontier_ns is None:
            return self._cache_empty(cache_key, context)
        setup = _setup(context)
        events = self.source.bbo(
            manifest_hash=setup.manifest_hash,
            instrument_id=setup.instrument_id,
            start_ns=context.start_ns,
            through_ns=context.frontier_ns,
            after_source_sequence=after_sequence,
            limit=limit,
        )
        visible = self._visible(events, context, after_sequence, limit)
        page = MarketPage(
            items=tuple(self._project_bbo(context, quote) for quote in visible),
            next_cursor=_next_cursor(visible, limit),
            frontier=self._frontier_payload(context),
        )
        self._cache[cache_key] = page
        return page

    def depth(
        self,
        *,
        session_id: str,
        principal_id: str,
        start_offset_ns: int = 0,
        after_sequence: int | None = None,
        limit: int = 100,
    ) -> MarketPage:
        """Return visible L2 snapshots without provider-calendar leakage."""
        context = self._context(session_id, principal_id, start_offset_ns)
        cache_key = self._cache_key("DEPTH", context, principal_id, after_sequence, limit, None)
        if cache_key in self._cache:
            return self._cache[cache_key]
        if context.frontier_ns is None:
            return self._cache_empty(cache_key, context)
        setup = _setup(context)
        events = self.source.depth(
            manifest_hash=setup.manifest_hash,
            instrument_id=setup.instrument_id,
            start_ns=context.start_ns,
            through_ns=context.frontier_ns,
            after_source_sequence=after_sequence,
            limit=limit,
        )
        visible = self._visible(events, context, after_sequence, limit)
        page = MarketPage(
            items=tuple(self._project_depth(context, snapshot) for snapshot in visible),
            next_cursor=_next_cursor(visible, limit),
            frontier=self._frontier_payload(context),
        )
        self._cache[cache_key] = page
        return page

    def candles(
        self,
        *,
        session_id: str,
        principal_id: str,
        interval_ns: int,
        start_offset_ns: int = 0,
        limit: int = 250,
    ) -> MarketPage:
        """Aggregate only already-visible trades into deterministic episode-anchored candles."""
        if interval_ns not in SUPPORTED_INTERVALS_NS:
            raise MarketServiceError(
                MarketErrorCode.UNSUPPORTED_INTERVAL,
                "interval_ns is not an allowed deterministic interval",
            )
        context = self._context(session_id, principal_id, start_offset_ns)
        cache_key = self._cache_key("CANDLES", context, principal_id, None, limit, interval_ns)
        if cache_key in self._cache:
            return self._cache[cache_key]
        if context.frontier_ns is None:
            return self._cache_empty(cache_key, context)
        buckets = self._candle_buckets(context, interval_ns, limit)
        items = tuple(
            self._project_candle(context, bucket, interval_ns, trades)
            for bucket, trades in sorted(buckets.items())[:limit]
        )
        page = MarketPage(
            items=items,
            next_cursor=None,
            frontier=self._frontier_payload(context),
        )
        self._cache[cache_key] = page
        return page

    def current_quote(self, *, session_id: str, principal_id: str) -> VisibleQuote | None:
        """Implement M3-06's quote resolver using only the visible replay frontier."""
        context = self._context(session_id, principal_id, 0)
        if context.frontier_ns is None:
            return None
        setup = _setup(context)
        quote = self.source.latest_bbo(
            manifest_hash=setup.manifest_hash,
            instrument_id=setup.instrument_id,
            start_ns=setup.play_start_ns - setup.warmup_ns,
            through_ns=context.frontier_ns,
        )
        if quote is None or quote.ts_ns > context.frontier_ns:
            return None
        if quote.ts_ns < setup.play_start_ns - setup.warmup_ns:
            return None
        return VisibleQuote(
            event_id=self._event_id(context.session.session_id, quote.source_event_id),
            bid_price_atoms=quote.bid_price_atoms,
            ask_price_atoms=quote.ask_price_atoms,
        )

    def _context(self, session_id: str, principal_id: str, start_offset_ns: int) -> _Context:
        _validate_query(start_offset_ns, None, 1)
        try:
            session = self.sessions.get_session(
                session_id=session_id,
                principal_id=principal_id,
            )
        except SessionLifecycleError as error:
            raise MarketServiceError(
                MarketErrorCode.SESSION_UNAVAILABLE,
                "session is unavailable",
            ) from error
        if session.setup is None:
            raise MarketServiceError(
                MarketErrorCode.SESSION_NOT_COMMITTED,
                "market data requires a committed setup",
            )
        start_ns = session.setup.play_start_ns + start_offset_ns
        if start_ns < session.setup.play_start_ns or start_ns >= session.setup.play_end_ns:
            raise MarketServiceError(
                MarketErrorCode.INVALID_QUERY,
                "start_offset_ns is outside the committed play interval",
            )
        if session.logical_time_ns is None:
            frontier = None
        else:
            frontier = min(session.logical_time_ns, session.setup.play_end_ns - 1)
            if frontier < session.setup.play_start_ns:
                frontier = None
        return _Context(session=session, frontier_ns=frontier, start_ns=start_ns)

    def _visible(
        self,
        events: Sequence[T],
        context: _Context,
        after_sequence: int | None,
        limit: int,
    ) -> tuple[T, ...]:
        _validate_query(0, after_sequence, limit)
        frontier = context.frontier_ns
        if frontier is None:
            return ()
        visible = tuple(
            event
            for event in events
            if context.start_ns <= event.ts_ns <= frontier
            and (after_sequence is None or event.source_sequence > after_sequence)
        )
        self._validate_source_order(visible, after_sequence)
        return visible[:limit]

    def _candle_buckets(
        self, context: _Context, interval_ns: int, limit: int
    ) -> dict[int, list[Trade]]:
        setup = _setup(context)
        frontier = context.frontier_ns
        if frontier is None:
            return {}
        buckets: dict[int, list[Trade]] = {}
        after_sequence: int | None = None
        while len(buckets) < limit:
            raw = self.source.trades(
                manifest_hash=setup.manifest_hash,
                instrument_id=setup.instrument_id,
                start_ns=context.start_ns,
                through_ns=frontier,
                after_source_sequence=after_sequence,
                limit=MAX_PAGE_SIZE,
            )
            if not raw:
                break
            self._validate_source_order(raw, after_sequence)
            for trade in raw:
                if context.start_ns <= trade.ts_ns <= frontier:
                    bucket = (trade.ts_ns - setup.play_start_ns) // interval_ns
                    buckets.setdefault(bucket, []).append(trade)
            next_after = raw[-1].source_sequence
            if after_sequence is not None and next_after <= after_sequence:
                raise MarketServiceError(
                    MarketErrorCode.SOURCE_ORDER,
                    "market source pagination did not advance",
                )
            after_sequence = next_after
            if len(raw) < MAX_PAGE_SIZE:
                break
        return buckets

    @staticmethod
    def _validate_source_order(events: Sequence[T], after_sequence: int | None) -> None:
        prior_sequence = after_sequence
        prior_ts: int | None = None
        for event in events:
            if prior_sequence is not None and event.source_sequence <= prior_sequence:
                raise MarketServiceError(
                    MarketErrorCode.SOURCE_ORDER,
                    "market source sequence is not strictly increasing",
                )
            if prior_ts is not None and event.ts_ns < prior_ts:
                raise MarketServiceError(
                    MarketErrorCode.SOURCE_ORDER,
                    "market source event time regressed",
                )
            prior_sequence = event.source_sequence
            prior_ts = event.ts_ns

    def _project_trade(self, context: _Context, trade: Trade) -> dict[str, object]:
        return {
            "event_id": self._event_id(context.session.session_id, trade.source_event_id),
            **self._time_payload(context, trade.ts_ns),
            "price_atoms": str(trade.price_atoms),
            "quantity_atoms": str(trade.quantity_atoms),
        }

    def _project_bbo(self, context: _Context, quote: Bbo) -> dict[str, object]:
        return {
            "event_id": self._event_id(context.session.session_id, quote.source_event_id),
            **self._time_payload(context, quote.ts_ns),
            "bid_price_atoms": str(quote.bid_price_atoms),
            "bid_quantity_atoms": str(quote.bid_quantity_atoms),
            "ask_price_atoms": str(quote.ask_price_atoms),
            "ask_quantity_atoms": str(quote.ask_quantity_atoms),
        }

    def _project_depth(self, context: _Context, snapshot: DepthSnapshot) -> dict[str, object]:
        return {
            "event_id": self._event_id(context.session.session_id, snapshot.source_event_id),
            **self._time_payload(context, snapshot.ts_ns),
            "bids": [
                {"price_atoms": str(level.price_atoms), "quantity_atoms": str(level.quantity_atoms)}
                for level in snapshot.bids
            ],
            "asks": [
                {"price_atoms": str(level.price_atoms), "quantity_atoms": str(level.quantity_atoms)}
                for level in snapshot.asks
            ],
        }

    def _project_candle(
        self,
        context: _Context,
        bucket: int,
        interval_ns: int,
        trades: Sequence[Trade],
    ) -> dict[str, object]:
        prices = [trade.price_atoms for trade in trades]
        volume = sum(trade.quantity_atoms for trade in trades)
        bucket_start_ns = _setup(context).play_start_ns + bucket * interval_ns
        return {
            **self._time_payload(context, bucket_start_ns),
            "open_atoms": str(prices[0]),
            "high_atoms": str(max(prices)),
            "low_atoms": str(min(prices)),
            "close_atoms": str(prices[-1]),
            "base_volume_atoms": str(volume),
            "trade_count": str(len(trades)),
        }

    def _time_payload(self, context: _Context, ts_ns: int) -> dict[str, str]:
        setup = _setup(context)
        if setup.visibility_mode is VisibilityMode.ABSOLUTE:
            return {"ts_ns": str(ts_ns)}
        return {"offset_ns": str(ts_ns - setup.play_start_ns)}

    def _frontier_payload(self, context: _Context) -> dict[str, str]:
        if context.frontier_ns is None:
            return {"offset_ns": "-1"}
        return self._time_payload(context, context.frontier_ns)

    def _cache_key(
        self,
        channel: str,
        context: _Context,
        principal_id: str,
        after_sequence: int | None,
        limit: int,
        interval_ns: int | None,
    ) -> tuple[object, ...]:
        _validate_query(0, after_sequence, limit)
        return (
            principal_id,
            context.session.session_id,
            context.session.version,
            context.frontier_ns,
            channel,
            context.start_ns,
            after_sequence,
            limit,
            interval_ns,
        )

    def _cache_empty(self, cache_key: tuple[object, ...], context: _Context) -> MarketPage:
        page = MarketPage(items=(), next_cursor=None, frontier=self._frontier_payload(context))
        self._cache[cache_key] = page
        return page

    @staticmethod
    def _event_id(session_id: str, source_event_id: str) -> str:
        digest = hashlib.sha256(
            b"TRL-VISIBLE-EVENT-v1\0"
            + session_id.encode("utf-8")
            + b"\0"
            + source_event_id.encode("utf-8")
        ).hexdigest()
        return f"evt_{digest}"


def _setup(context: _Context) -> CommittedSetup:
    setup = context.session.setup
    if setup is None:
        raise RuntimeError("market context lost committed setup")
    return setup


def _validate_query(start_offset_ns: int, after_sequence: int | None, limit: int) -> None:
    if isinstance(start_offset_ns, bool) or start_offset_ns < 0:
        raise MarketServiceError(
            MarketErrorCode.INVALID_QUERY,
            "start_offset_ns must be a nonnegative integer",
        )
    if after_sequence is not None and (isinstance(after_sequence, bool) or after_sequence < 0):
        raise MarketServiceError(
            MarketErrorCode.INVALID_QUERY,
            "after_sequence must be a nonnegative integer",
        )
    if isinstance(limit, bool) or not 1 <= limit <= MAX_PAGE_SIZE:
        raise MarketServiceError(
            MarketErrorCode.INVALID_QUERY,
            f"limit must be from 1 through {MAX_PAGE_SIZE}",
        )


def _next_cursor(events: Sequence[T], limit: int) -> str | None:
    if len(events) < limit or not events:
        return None
    return str(events[-1].source_sequence)


__all__ = ["MAX_PAGE_SIZE", "MarketPage", "MarketService", "SUPPORTED_INTERVALS_NS"]
