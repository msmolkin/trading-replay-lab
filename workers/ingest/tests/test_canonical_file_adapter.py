from pathlib import Path
from typing import Literal

import pyarrow as pa
import pytest
from pyarrow import parquet as pq

from trading_replay_ingest.adapters.canonical_file import (
    CanonicalFileAdapter,
    ColumnMapping,
    ImportDeclaration,
    ImportLimits,
    PathRejected,
    ResourceLimitExceeded,
    SchemaRejected,
)
from trading_replay_ingest.core import FetchRequest
from trading_replay_ingest.core.canonical import JsonValue


MAPPINGS = (
    ColumnMapping("ts", "ts_event_ns"),
    ColumnMapping("seq", "canonical_tie_breaker"),
    ColumnMapping("kind", "kind"),
    ColumnMapping("price", "payload.price_atoms"),
    ColumnMapping("qty", "payload.qty_atoms"),
    ColumnMapping("side", "payload.aggressor_side"),
)
DEFAULTS: tuple[tuple[str, JsonValue], ...] = (
    ("schema_version", "1.0.0"),
    ("dataset_id", "user-file"),
    ("instrument_id", "SYNTH"),
    ("venue_id", "USER"),
    ("quality_flags", []),
)


def request() -> FetchRequest:
    return FetchRequest("canonical_file", "trades", "SYNTH", 0, 100)


def declaration(format_name: Literal["csv", "parquet"]) -> ImportDeclaration:
    return ImportDeclaration(
        format=format_name,
        mappings=MAPPINGS,
        defaults=DEFAULTS,
        capabilities=("TRADES",),
        provenance="unit-test user fixture",
    )


def adapter(
    root: Path, source: str, format_name: Literal["csv", "parquet"]
) -> CanonicalFileAdapter:
    return CanonicalFileAdapter(
        root=root,
        source_path=Path(source),
        declaration=declaration(format_name),
        limits=ImportLimits(
            max_file_bytes=1_000_000,
            max_rows=10,
            max_columns=10,
            max_materialized_bytes=1_000_000,
        ),
    )


def test_csv_import_maps_exact_integer_strings(tmp_path: Path) -> None:
    root = tmp_path / "root"
    root.mkdir()
    source = root / "trades.csv"
    source.write_text("ts,seq,kind,price,qty,side\n10,0,TRADE,100,2,BUY\n", encoding="utf-8")
    importer = adapter(root, "trades.csv", "csv")
    chunk = importer.plan(request()).chunks[0]
    result = importer.normalize(chunk, importer.fetch(chunk))
    event = result.events[0]
    payload = event["payload"]
    assert isinstance(payload, dict)
    assert payload["price_atoms"] == "100"
    assert payload["qty_atoms"] == "2"
    assert importer.declaration.capabilities == ("TRADES",)


def test_parquet_import_maps_integers_without_float_roundtrip(tmp_path: Path) -> None:
    root = tmp_path / "root"
    root.mkdir()
    source = root / "trades.parquet"
    table = pa.table(
        {
            "ts": [10],
            "seq": [0],
            "kind": ["TRADE"],
            "price": [100],
            "qty": [2],
            "side": ["BUY"],
        }
    )
    pq.write_table(table, source)
    importer = adapter(root, "trades.parquet", "parquet")
    chunk = importer.plan(request()).chunks[0]
    event = importer.normalize(chunk, importer.fetch(chunk)).events[0]
    payload = event["payload"]
    assert isinstance(payload, dict)
    assert payload["price_atoms"] == "100"


def test_path_traversal_is_rejected(tmp_path: Path) -> None:
    root = tmp_path / "root"
    root.mkdir()
    (tmp_path / "outside.csv").write_text("x\n1\n", encoding="utf-8")
    with pytest.raises(PathRejected):
        adapter(root, "../outside.csv", "csv")


def test_archive_container_is_rejected_before_decompression(tmp_path: Path) -> None:
    root = tmp_path / "root"
    root.mkdir()
    source = root / "pretend.csv"
    source.write_bytes(b"PK\x03\x04" + b"x" * 100)
    importer = adapter(root, "pretend.csv", "csv")
    chunk = importer.plan(request()).chunks[0]
    with pytest.raises(ResourceLimitExceeded, match="archive"):
        importer.fetch(chunk)


def test_undeclared_csv_column_fails_closed(tmp_path: Path) -> None:
    root = tmp_path / "root"
    root.mkdir()
    source = root / "trades.csv"
    source.write_text(
        "ts,seq,kind,price,qty,side,secret\n10,0,TRADE,100,2,BUY,nope\n",
        encoding="utf-8",
    )
    importer = adapter(root, "trades.csv", "csv")
    chunk = importer.plan(request()).chunks[0]
    with pytest.raises(SchemaRejected, match="exactly match"):
        importer.normalize(chunk, importer.fetch(chunk))


def test_parquet_float_money_fails_closed(tmp_path: Path) -> None:
    root = tmp_path / "root"
    root.mkdir()
    source = root / "trades.parquet"
    pq.write_table(
        pa.table(
            {
                "ts": [10],
                "seq": [0],
                "kind": ["TRADE"],
                "price": [100.5],
                "qty": [2],
                "side": ["BUY"],
            }
        ),
        source,
    )
    importer = adapter(root, "trades.parquet", "parquet")
    chunk = importer.plan(request()).chunks[0]
    with pytest.raises(SchemaRejected, match="floating-point"):
        importer.normalize(chunk, importer.fetch(chunk))
