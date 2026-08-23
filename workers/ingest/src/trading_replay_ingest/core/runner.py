"""Resumable provider-neutral ingestion runner."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .budget import BudgetGuard
from .cache import ContentAddressedCache
from .canonical import JsonValue
from .model import Adapter, FetchRequest
from .state import JobStateStore
from .writer import write_canonical


@dataclass(frozen=True, slots=True)
class RunResult:
    """Stable identifiers emitted by a completed ingestion job."""

    job_id: str
    content_sha256: str
    row_count: int


class IngestionRunner:
    """Executes a deterministic fetch plan with checkpointed raw chunks."""

    def __init__(
        self,
        *,
        cache: ContentAddressedCache,
        state: JobStateStore,
        budget: BudgetGuard,
    ) -> None:
        self.cache = cache
        self.state = state
        self.budget = budget

    def run(
        self,
        adapter: Adapter,
        request: FetchRequest,
        *,
        output_path: Path,
        manifest_path: Path,
    ) -> RunResult:
        """Execute or resume one idempotent ingestion job."""
        plan = adapter.plan(request)
        self.budget.preflight(
            requests=len(plan.chunks),
            cost_minor=plan.estimated_cost_minor,
        )
        checkpoint = self.state.load(request.job_id)
        events: list[dict[str, JsonValue]] = []
        hashes: list[tuple[str, str]] = []

        for chunk in plan.chunks:
            digest = checkpoint.completed.get(chunk.key)
            if digest is None:
                raw = adapter.fetch(chunk)
                self.budget.consume(
                    bytes_count=len(raw),
                    cost_minor=chunk.estimated_cost_minor,
                )
                digest = self.cache.put(raw)
                checkpoint.completed[chunk.key] = digest
                self.state.save(checkpoint)
            else:
                raw = self.cache.get(digest)
            batch = adapter.normalize(chunk, raw)
            events.extend(batch.events)
            hashes.append((chunk.key, digest))

        content_hash = write_canonical(
            output_path,
            manifest_path,
            job_id=request.job_id,
            events=events,
            chunk_hashes=hashes,
        )
        return RunResult(request.job_id, content_hash, len(events))
