from __future__ import annotations

import json
from pathlib import Path
from typing import cast

import pytest

from trading_replay_ingest.core import (
    BudgetExceeded,
    BudgetGuard,
    ContentAddressedCache,
    FetchChunk,
    FetchPlan,
    FetchRequest,
    IngestionRunner,
    JobStateStore,
    NormalizedBatch,
)
from trading_replay_ingest.core.canonical import JsonValue


class FakeAdapter:
    def __init__(self, *, fail_once_on: str | None = None) -> None:
        self.fail_once_on = fail_once_on
        self.failed = False
        self.fetch_count: dict[str, int] = {}
        self.raw = {"a": b"100\n", "b": b"101\n"}

    def plan(self, request: FetchRequest) -> FetchPlan:
        del request
        return FetchPlan(
            (
                FetchChunk("a", "memory://a", estimated_cost_minor=2),
                FetchChunk("b", "memory://b", estimated_cost_minor=2),
            )
        )

    def fetch(self, chunk: FetchChunk) -> bytes:
        self.fetch_count[chunk.key] = self.fetch_count.get(chunk.key, 0) + 1
        if self.fail_once_on == chunk.key and not self.failed:
            self.failed = True
            raise RuntimeError("synthetic interruption")
        return self.raw[chunk.key]

    def normalize(self, chunk: FetchChunk, raw: bytes) -> NormalizedBatch:
        price = raw.decode("ascii").strip()
        event: dict[str, JsonValue] = {
            "schema_version": "1.0.0",
            "dataset_id": "fake",
            "instrument_id": "SYNTH",
            "venue_id": "TEST",
            "ts_event_ns": str(10 if chunk.key == "a" else 20),
            "canonical_tie_breaker": "0",
            "kind": "TRADE",
            "payload": {
                "price_atoms": price,
                "qty_atoms": "1",
                "aggressor_side": "UNKNOWN",
            },
            "quality_flags": [],
        }
        return NormalizedBatch((event,))


def request() -> FetchRequest:
    return FetchRequest("fake", "trades", "SYNTH", 0, 100)


def runner(root: Path) -> IngestionRunner:
    return IngestionRunner(
        cache=ContentAddressedCache(root / "cache"),
        state=JobStateStore(root / "state"),
        budget=BudgetGuard(max_requests=10, max_bytes=1_000, max_cost_minor=100),
    )


def test_interrupted_job_resumes_and_matches_clean_run(tmp_path: Path) -> None:
    interrupted_root = tmp_path / "interrupted"
    flaky = FakeAdapter(fail_once_on="b")
    with pytest.raises(RuntimeError, match="synthetic interruption"):
        runner(interrupted_root).run(
            flaky,
            request(),
            output_path=interrupted_root / "events.jsonl",
            manifest_path=interrupted_root / "manifest.json",
        )
    assert flaky.fetch_count == {"a": 1, "b": 1}

    resumed = runner(interrupted_root).run(
        flaky,
        request(),
        output_path=interrupted_root / "events.jsonl",
        manifest_path=interrupted_root / "manifest.json",
    )
    assert flaky.fetch_count == {"a": 1, "b": 2}

    clean_root = tmp_path / "clean"
    clean = FakeAdapter()
    clean_result = runner(clean_root).run(
        clean,
        request(),
        output_path=clean_root / "events.jsonl",
        manifest_path=clean_root / "manifest.json",
    )

    assert resumed == clean_result
    assert (interrupted_root / "events.jsonl").read_bytes() == (
        clean_root / "events.jsonl"
    ).read_bytes()
    assert (interrupted_root / "manifest.json").read_bytes() == (
        clean_root / "manifest.json"
    ).read_bytes()


def test_budget_fails_before_mutating_counters() -> None:
    budget = BudgetGuard(max_requests=1, max_bytes=3, max_cost_minor=2)
    with pytest.raises(BudgetExceeded):
        budget.consume(bytes_count=4, cost_minor=1)
    assert (budget.requests, budget.bytes, budget.cost_minor) == (0, 0, 0)


def test_canonical_writer_rejects_float(tmp_path: Path) -> None:
    adapter = FakeAdapter()
    original = adapter.normalize

    def bad_normalize(chunk: FetchChunk, raw: bytes) -> NormalizedBatch:
        batch = original(chunk, raw)
        event = dict(batch.events[0])
        event["payload"] = cast(JsonValue, {"price_atoms": 1.5})
        return NormalizedBatch((event,))

    adapter.normalize = bad_normalize  # type: ignore[method-assign]
    with pytest.raises(TypeError, match="floating-point"):
        runner(tmp_path).run(
            adapter,
            request(),
            output_path=tmp_path / "events.jsonl",
            manifest_path=tmp_path / "manifest.json",
        )


def test_checkpoint_is_stable_json(tmp_path: Path) -> None:
    adapter = FakeAdapter()
    runner(tmp_path).run(
        adapter,
        request(),
        output_path=tmp_path / "events.jsonl",
        manifest_path=tmp_path / "manifest.json",
    )
    checkpoint_path = tmp_path / "state" / f"{request().job_id}.json"
    value = json.loads(checkpoint_path.read_text(encoding="utf-8"))
    assert list(value) == ["completed", "job_id"]
