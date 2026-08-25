"""Injectable FastAPI routes for visibility-gated market reads."""

from __future__ import annotations

from collections.abc import Callable
from typing import Annotated, cast

from fastapi import APIRouter, HTTPException, Query, Request

from trading_replay_api.auth import AuthenticationError, Authenticator

from .model import MarketErrorCode, MarketServiceError
from .service import MarketPage, MarketService


def build_market_router(*, service: MarketService, authenticator: Authenticator) -> APIRouter:
    """Build principal-scoped market routes without global service state."""
    router = APIRouter(prefix="/sessions/{session_id}/market", tags=["market"])

    @router.get("/trades")
    def trades(
        session_id: str,
        request: Request,
        start_offset_ns: Annotated[str, Query()] = "0",
        after_sequence: Annotated[str | None, Query()] = None,
        limit: Annotated[int, Query(ge=1, le=1_000)] = 250,
    ) -> dict[str, object]:
        principal_id = _authenticate(authenticator, request)
        return _page(
            _invoke(
                service.trades,
                session_id=session_id,
                principal_id=principal_id,
                start_offset_ns=_nonnegative_decimal(start_offset_ns, "start_offset_ns"),
                after_sequence=_optional_decimal(after_sequence, "after_sequence"),
                limit=limit,
            )
        )

    @router.get("/bbo")
    def bbo(
        session_id: str,
        request: Request,
        start_offset_ns: Annotated[str, Query()] = "0",
        after_sequence: Annotated[str | None, Query()] = None,
        limit: Annotated[int, Query(ge=1, le=1_000)] = 250,
    ) -> dict[str, object]:
        principal_id = _authenticate(authenticator, request)
        return _page(
            _invoke(
                service.bbo,
                session_id=session_id,
                principal_id=principal_id,
                start_offset_ns=_nonnegative_decimal(start_offset_ns, "start_offset_ns"),
                after_sequence=_optional_decimal(after_sequence, "after_sequence"),
                limit=limit,
            )
        )

    @router.get("/depth")
    def depth(
        session_id: str,
        request: Request,
        start_offset_ns: Annotated[str, Query()] = "0",
        after_sequence: Annotated[str | None, Query()] = None,
        limit: Annotated[int, Query(ge=1, le=1_000)] = 100,
    ) -> dict[str, object]:
        principal_id = _authenticate(authenticator, request)
        return _page(
            _invoke(
                service.depth,
                session_id=session_id,
                principal_id=principal_id,
                start_offset_ns=_nonnegative_decimal(start_offset_ns, "start_offset_ns"),
                after_sequence=_optional_decimal(after_sequence, "after_sequence"),
                limit=limit,
            )
        )

    @router.get("/candles")
    def candles(
        session_id: str,
        request: Request,
        interval_ns: Annotated[str, Query()],
        start_offset_ns: Annotated[str, Query()] = "0",
        limit: Annotated[int, Query(ge=1, le=1_000)] = 250,
    ) -> dict[str, object]:
        principal_id = _authenticate(authenticator, request)
        return _page(
            _invoke(
                service.candles,
                session_id=session_id,
                principal_id=principal_id,
                interval_ns=_positive_decimal(interval_ns, "interval_ns"),
                start_offset_ns=_nonnegative_decimal(start_offset_ns, "start_offset_ns"),
                limit=limit,
            )
        )

    return router


def _authenticate(authenticator: Authenticator, request: Request) -> str:
    try:
        return authenticator.authenticate(request.headers).principal_id
    except AuthenticationError as error:
        raise HTTPException(status_code=401, detail="authentication required") from error


def _invoke(function: object, /, **kwargs: object) -> MarketPage:
    try:
        if not callable(function):
            raise RuntimeError("market service method is not callable")
        callable_function = cast(Callable[..., object], function)
        result = callable_function(**kwargs)
        if not isinstance(result, MarketPage):
            raise RuntimeError("market service returned unexpected result")
        return result
    except MarketServiceError as error:
        raise _http_error(error) from error


def _http_error(error: MarketServiceError) -> HTTPException:
    status = {
        MarketErrorCode.SESSION_UNAVAILABLE: 404,
        MarketErrorCode.SESSION_NOT_COMMITTED: 409,
        MarketErrorCode.INVALID_QUERY: 422,
        MarketErrorCode.UNSUPPORTED_INTERVAL: 422,
        MarketErrorCode.SOURCE_ORDER: 500,
    }[error.code]
    return HTTPException(
        status_code=status,
        detail={"code": error.code.value, "message": str(error)},
    )


def _page(page: MarketPage) -> dict[str, object]:
    return {
        "items": list(page.items),
        "next_cursor": page.next_cursor,
        "frontier": page.frontier,
    }


def _optional_decimal(value: str | None, name: str) -> int | None:
    if value is None:
        return None
    return _nonnegative_decimal(value, name)


def _positive_decimal(value: str, name: str) -> int:
    parsed = _nonnegative_decimal(value, name)
    if parsed == 0:
        raise _query_error(f"{name} must be positive")
    return parsed


def _nonnegative_decimal(value: str, name: str) -> int:
    if (
        not value
        or value.startswith(("+", "-"))
        or (value.startswith("0") and value != "0")
        or not value.isascii()
        or not value.isdigit()
    ):
        raise _query_error(f"{name} must be canonical nonnegative decimal text")
    parsed = int(value)
    if parsed > 2**64 - 1:
        raise _query_error(f"{name} exceeds unsigned 64-bit range")
    return parsed


def _query_error(message: str) -> HTTPException:
    return HTTPException(
        status_code=422,
        detail={"code": MarketErrorCode.INVALID_QUERY.value, "message": message},
    )


__all__ = ["build_market_router"]
