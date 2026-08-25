"""Visibility-gated market read boundary."""

from .model import (
    Bbo,
    DepthLevel,
    DepthSnapshot,
    MarketChannel,
    MarketDataSource,
    MarketErrorCode,
    MarketServiceError,
    SessionReader,
    Trade,
)
from .router import build_market_router
from .service import MAX_PAGE_SIZE, SUPPORTED_INTERVALS_NS, MarketPage, MarketService

__all__ = [
    "MAX_PAGE_SIZE",
    "SUPPORTED_INTERVALS_NS",
    "Bbo",
    "DepthLevel",
    "DepthSnapshot",
    "MarketChannel",
    "MarketDataSource",
    "MarketErrorCode",
    "MarketPage",
    "MarketService",
    "MarketServiceError",
    "SessionReader",
    "Trade",
    "build_market_router",
]
