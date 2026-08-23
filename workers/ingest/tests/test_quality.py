from __future__ import annotations

import json
from collections.abc import Mapping
from pathlib import Path
from typing import cast

from trading_replay_ingest.quality import QualityPolicy, validate_events


def policy() -> QualityPolicy:
    return QualityPolicy(
        tick_size_atoms=5,
        qty_increment_atoms=1,
        max_gap_ns=100,
        max_quote_staleness_ns=50,
    )


def trade(ts: int, sequence: int, price: int = 100, qty: int = 1) -> Mapping[str, object]:
    return {
        "schema_version": "1.0.0",
        "ts_event_ns": str(ts),
        "source_sequence": str(sequence),
        "kind": "TRADE",
        "payload": {
            "price_atoms": str(price),
            "qty_atoms": str(qty),
            "aggressor_side": "UNKNOWN",
        },
    }


def test_clean_report_is_valid_and_byte_stable() -> None:
    events = [trade(10, 1), trade(20, 2)]
    first = validate_events(events, policy())
    second = validate_events(events, policy())
    assert first.status == "VALID"
    assert first.row_count == 2
    assert first.issues == ()
    assert first.decision_hash == second.decision_hash


def test_duplicate_gap_and_sequence_gap_degrade_trade_data() -> None:
    first = trade(10, 1)
    report = validate_events([first, first, trade(250, 4)], policy())
    assert report.status == "DEGRADED"
    assert report.duplicates == 1
    assert {issue.code for issue in report.issues} >= {
        "DUPLICATE",
        "TIME_GAP",
        "SOURCE_SEQUENCE_GAP",
    }


def test_invalid_increment_quarantines() -> None:
    report = validate_events([trade(10, 1, price=103)], policy())
    assert report.status == "QUARANTINED"
    assert any(issue.code == "PRICE_INCREMENT" for issue in report.issues)


def test_delta_without_snapshot_quarantines() -> None:
    report = validate_events(
        [
            {
                "ts_event_ns": "10",
                "source_sequence": "1",
                "kind": "BOOK_DELTA_L2",
                "payload": {
                    "side": "BUY",
                    "price_atoms": "100",
                    "new_qty_atoms": "1",
                    "action": "UPSERT",
                },
            }
        ],
        policy(),
    )
    assert report.status == "QUARANTINED"
    assert any(issue.code == "DELTA_WITHOUT_SNAPSHOT" for issue in report.issues)


def test_crossed_book_quarantines() -> None:
    report = validate_events(
        [
            {
                "ts_event_ns": "10",
                "source_sequence": "1",
                "kind": "BOOK_SNAPSHOT_L2",
                "payload": {
                    "bids": [{"price_atoms": "105", "qty_atoms": "1"}],
                    "asks": [{"price_atoms": "100", "qty_atoms": "1"}],
                    "scope": "FULL",
                },
            }
        ],
        policy(),
    )
    assert report.status == "QUARANTINED"
    assert any(issue.code == "CROSSED_BOOK" for issue in report.issues)


def test_committed_f2_gap_fixture_is_quarantined_reproducibly() -> None:
    root = Path(__file__).resolve().parents[3]
    raw_events = [
        json.loads(line)
        for line in (root / "fixtures" / "micro" / "f2.jsonl")
        .read_text(encoding="utf-8")
        .splitlines()
        if line
    ]
    events = cast(list[Mapping[str, object]], raw_events)
    fixture_policy = QualityPolicy(
        tick_size_atoms=5,
        qty_increment_atoms=1,
        max_gap_ns=10_000_000_000,
        max_quote_staleness_ns=10_000_000_000,
    )
    first = validate_events(events, fixture_policy)
    second = validate_events(events, fixture_policy)
    assert first.status == "QUARANTINED"
    assert any(issue.code == "BOOK_SEQUENCE_GAP" for issue in first.issues)
    assert first.decision_hash == second.decision_hash
