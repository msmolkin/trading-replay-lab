"""Database schema and event-store transaction boundary."""

from .schema import metadata
from .store import (
    ZERO_HASH,
    AppendResult,
    CommandRecord,
    ConcurrentSessionVersion,
    EventChainConflict,
    EventRecord,
    EventStore,
    EventStoreError,
    IdempotencyConflict,
    SessionNotFound,
)

__all__ = [
    "ZERO_HASH",
    "AppendResult",
    "CommandRecord",
    "ConcurrentSessionVersion",
    "EventChainConflict",
    "EventRecord",
    "EventStore",
    "EventStoreError",
    "IdempotencyConflict",
    "SessionNotFound",
    "metadata",
]
