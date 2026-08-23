#!/usr/bin/env python3
"""Generate deterministic, license-safe market micro-fixtures."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "fixtures" / "micro"
BASE_NS = 1_700_000_000_000_000_000


def event(dataset: str, kind: str, ts_event_ns: int, tie: int, payload: dict[str, object], *, source_sequence: int | None = None, source_event_id: str | None = None, quality_flags: list[str] | None = None) -> dict[str, object]:
    value: dict[str, object] = {"schema_version": "1.0.0", "dataset_id": dataset, "instrument_id": "SYNTH-BTC-USD", "venue_id": "TRL-SYNTH", "ts_event_ns": str(ts_event_ns), "canonical_tie_breaker": str(tie), "kind": kind, "payload": payload, "quality_flags": quality_flags or []}
    if source_sequence is not None:
        value["source_sequence"] = str(source_sequence)
    if source_event_id is not None:
        value["source_event_id"] = source_event_id
    return value


def fixtures() -> dict[str, list[dict[str, object]]]:
    f0 = [
        event("fixture-f0", "BAR", BASE_NS, 0, {"interval": "1m", "open_atoms": "10000", "high_atoms": "10300", "low_atoms": "9700", "close_atoms": "10100", "base_volume_atoms": "500", "trade_count": "5", "complete": True}, source_event_id="f0-bar-0"),
        event("fixture-f0", "BAR", BASE_NS + 60_000_000_000, 0, {"interval": "1m", "open_atoms": "10100", "high_atoms": "10500", "low_atoms": "9900", "close_atoms": "10400", "base_volume_atoms": "700", "trade_count": "7", "complete": True}, source_event_id="f0-bar-1"),
        event("fixture-f0", "BAR", BASE_NS + 120_000_000_000, 0, {"interval": "1m", "open_atoms": "10400", "high_atoms": "10800", "low_atoms": "9200", "close_atoms": "9500", "base_volume_atoms": "900", "trade_count": "9", "complete": True}, source_event_id="f0-bar-ambiguous", quality_flags=["INTRABAR_AMBIGUOUS"]),
    ]
    f1 = [
        event("fixture-f1", "BBO", BASE_NS, 0, {"bid_price_atoms": "9990", "bid_qty_atoms": "20", "ask_price_atoms": "10010", "ask_qty_atoms": "15"}, source_sequence=100, source_event_id="f1-bbo-100"),
        event("fixture-f1", "TRADE", BASE_NS, 1, {"price_atoms": "10010", "qty_atoms": "4", "aggressor_side": "BUY", "trade_id": "t-101"}, source_sequence=101, source_event_id="f1-trade-101"),
        event("fixture-f1", "BBO", BASE_NS + 1_000_000, 0, {"bid_price_atoms": "10000", "bid_qty_atoms": "10", "ask_price_atoms": "10020", "ask_qty_atoms": "9"}, source_sequence=102, source_event_id="f1-bbo-102"),
        event("fixture-f1", "TRADE", BASE_NS + 2_000_000, 0, {"price_atoms": "10000", "qty_atoms": "6", "aggressor_side": "SELL", "trade_id": "t-104"}, source_sequence=104, source_event_id="f1-trade-gap", quality_flags=["SEQUENCE_GAP"]),
    ]
    f2 = [
        event("fixture-f2", "BOOK_SNAPSHOT_L2", BASE_NS, 0, {"bids": [{"price_atoms": "9990", "qty_atoms": "12"}, {"price_atoms": "9980", "qty_atoms": "20"}], "asks": [{"price_atoms": "10010", "qty_atoms": "8"}, {"price_atoms": "10020", "qty_atoms": "14"}], "scope": "TOP_N", "depth": 2}, source_sequence=200, source_event_id="f2-snap-200"),
        event("fixture-f2", "BOOK_DELTA_L2", BASE_NS + 1_000_000, 0, {"side": "SELL", "price_atoms": "10010", "new_qty_atoms": "3", "action": "UPSERT"}, source_sequence=201, source_event_id="f2-delta-201"),
        event("fixture-f2", "BOOK_DELTA_L2", BASE_NS + 1_000_000, 1, {"side": "BUY", "price_atoms": "9990", "new_qty_atoms": "7", "action": "UPSERT"}, source_sequence=202, source_event_id="f2-delta-202"),
        event("fixture-f2", "BOOK_DELTA_L2", BASE_NS + 2_000_000, 0, {"side": "SELL", "price_atoms": "10010", "new_qty_atoms": "0", "action": "DELETE"}, source_sequence=204, source_event_id="f2-gap-204", quality_flags=["SEQUENCE_GAP"]),
        event("fixture-f2", "BOOK_SNAPSHOT_L2", BASE_NS + 3_000_000, 0, {"bids": [{"price_atoms": "9980", "qty_atoms": "18"}], "asks": [{"price_atoms": "10020", "qty_atoms": "10"}], "scope": "TOP_N", "depth": 1}, source_sequence=205, source_event_id="f2-resync-205"),
    ]
    edge = [
        event("fixture-edge", "FUNDING_RATE", BASE_NS + 3_600_000_000_000, 0, {"value_atoms": "25000", "unit": "rate_ppb"}, source_event_id="funding-1"),
        event("fixture-edge", "CORPORATE_ACTION", BASE_NS + 7_200_000_000_000, 0, {"action": "SPLIT", "numerator": "2", "denominator": "1", "effective_ns": str(BASE_NS + 7_200_000_000_000)}, source_event_id="split-1"),
    ]
    return {"f0.jsonl": f0, "f1.jsonl": f1, "f2.jsonl": f2, "edge-events.jsonl": edge}


def canonical_jsonl(rows: list[dict[str, object]]) -> str:
    return "".join(json.dumps(row, sort_keys=True, separators=(",", ":")) + "\n" for row in rows)


def expected_outputs() -> dict[Path, str]:
    outputs = {OUT / name: canonical_jsonl(rows) for name, rows in fixtures().items()}
    manifest = {
        "fixture_version": "1",
        "license": "CC0-1.0",
        "provenance": "Generated deterministically by tools/fixture-generator/generate.py; no market data source was used.",
        "instrument": {"instrument_id": "SYNTH-BTC-USD", "venue_id": "TRL-SYNTH", "price_scale": 2, "qty_scale": 0, "tick_size_atoms": "10", "qty_increment_atoms": "1"},
        "files": [{"path": path.name, "sha256": hashlib.sha256(content.encode()).hexdigest(), "rows": content.count("\n")} for path, content in sorted(outputs.items(), key=lambda item: item[0].name)],
        "coverage": ["timestamp_ties", "sequence_gap", "partial_depth", "funding", "corporate_action", "ambiguous_bar"],
    }
    outputs[OUT / "manifest.json"] = json.dumps(manifest, sort_keys=True, indent=2) + "\n"
    return outputs


def validate_semantics() -> None:
    data = fixtures()
    assert data["f1.jsonl"][0]["ts_event_ns"] == data["f1.jsonl"][1]["ts_event_ns"]
    assert data["f1.jsonl"][0]["canonical_tie_breaker"] == "0"
    assert data["f1.jsonl"][1]["canonical_tie_breaker"] == "1"
    assert any("SEQUENCE_GAP" in row["quality_flags"] for row in data["f1.jsonl"])
    assert any("SEQUENCE_GAP" in row["quality_flags"] for row in data["f2.jsonl"])
    assert any(row["kind"] == "FUNDING_RATE" for row in data["edge-events.jsonl"])
    assert any(row["kind"] == "CORPORATE_ACTION" for row in data["edge-events.jsonl"])
    assert any("INTRABAR_AMBIGUOUS" in row["quality_flags"] for row in data["f0.jsonl"])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    validate_semantics()
    outputs = expected_outputs()
    if args.check:
        drift = [path.relative_to(ROOT).as_posix() for path, expected in outputs.items() if not path.exists() or path.read_text(encoding="utf-8") != expected]
        if drift:
            print("Fixture drift:")
            for path in drift:
                print(f"- {path}")
            return 1
        print("Synthetic fixtures: byte-identical")
        return 0
    for path, content in outputs.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8", newline="\n")
    print("Synthetic fixtures generated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
