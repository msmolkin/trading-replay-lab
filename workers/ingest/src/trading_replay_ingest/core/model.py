"""Provider-neutral ingestion planning contracts."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass, field
from typing import Protocol

from .canonical import JsonValue, canonical_bytes


@dataclass(frozen=True, slots=True)
class FetchRequest:
    """Immutable provider-neutral request used to derive an idempotency key."""

    provider: str
    dataset: str
    instrument_id: str
    start_ns: int
    end_ns: int
    options: tuple[tuple[str, str], ...] = field(default_factory=tuple)

    def canonical(self) -> dict[str, JsonValue]:
        """Return the stable request representation used for hashing."""
        return {
            "provider": self.provider,
            "dataset": self.dataset,
            "instrument_id": self.instrument_id,
            "start_ns": str(self.start_ns),
            "end_ns": str(self.end_ns),
            "options": [[key, value] for key, value in sorted(self.options)],
        }

    @property
    def job_id(self) -> str:
        """Return a deterministic content-derived ingestion job identifier."""
        return hashlib.sha256(canonical_bytes(self.canonical())).hexdigest()


@dataclass(frozen=True, slots=True)
class FetchChunk:
    """One independently resumable provider object/range."""

    key: str
    source_ref: str
    expected_bytes: int | None = None
    estimated_cost_minor: int = 0


@dataclass(frozen=True, slots=True)
class FetchPlan:
    """Ordered chunks plus the plan's declared worst-case request cost."""

    chunks: tuple[FetchChunk, ...]

    @property
    def estimated_cost_minor(self) -> int:
        """Return the sum of per-chunk estimated costs."""
        return sum(chunk.estimated_cost_minor for chunk in self.chunks)


@dataclass(frozen=True, slots=True)
class NormalizedBatch:
    """Canonical records produced from one raw chunk."""

    events: tuple[dict[str, JsonValue], ...]


class Adapter(Protocol):
    """Minimal provider adapter boundary used by the ingestion runner."""

    def plan(self, request: FetchRequest) -> FetchPlan:
        """Build an ordered fetch plan without fetching data."""
        ...

    def fetch(self, chunk: FetchChunk) -> bytes:
        """Fetch one raw chunk."""
        ...

    def normalize(self, chunk: FetchChunk, raw: bytes) -> NormalizedBatch:
        """Normalize raw provider bytes into canonical records."""
        ...
