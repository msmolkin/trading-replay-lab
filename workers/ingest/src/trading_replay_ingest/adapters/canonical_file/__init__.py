"""Sandboxed canonical CSV/Parquet import adapter."""

from __future__ import annotations

import csv
import io
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

import pyarrow.parquet as pq

from trading_replay_ingest.core import FetchChunk, FetchPlan, FetchRequest, NormalizedBatch
from trading_replay_ingest.core.canonical import JsonValue


class CanonicalFileError(ValueError):
    """Base class for safe import failures."""


class PathRejected(CanonicalFileError):
    """Raised when an import escapes its declared sandbox root."""


class ResourceLimitExceeded(CanonicalFileError):
    """Raised before a source can exceed configured importer limits."""


class SchemaRejected(CanonicalFileError):
    """Raised for undeclared columns or values that violate canonical encoding."""


@dataclass(frozen=True, slots=True)
class ImportLimits:
    """Hard resource ceilings for untrusted user-supplied files."""

    max_file_bytes: int = 64 * 1024 * 1024
    max_rows: int = 1_000_000
    max_columns: int = 128
    max_materialized_bytes: int = 256 * 1024 * 1024

    def __post_init__(self) -> None:
        if (
            min(
                self.max_file_bytes,
                self.max_rows,
                self.max_columns,
                self.max_materialized_bytes,
            )
            <= 0
        ):
            raise ValueError("all import limits must be positive")


@dataclass(frozen=True, slots=True)
class ColumnMapping:
    """Maps one declared source column to a canonical dotted target path."""

    source: str
    target: str

    def __post_init__(self) -> None:
        if not self.source or not self.target:
            raise ValueError("column mapping names cannot be empty")
        if self.target.count(".") > 1:
            raise ValueError("only top-level or payload.* targets are supported")
        if "." in self.target and not self.target.startswith("payload."):
            raise ValueError("nested targets must live under payload")


@dataclass(frozen=True, slots=True)
class ImportDeclaration:
    """Explicit mapping, capability, and provenance contract for one import."""

    format: Literal["csv", "parquet"]
    mappings: tuple[ColumnMapping, ...]
    defaults: tuple[tuple[str, JsonValue], ...] = ()
    capabilities: tuple[str, ...] = ()
    provenance: str = "user-provided"
    redistribution_class: str = "USER_LICENSED"

    def __post_init__(self) -> None:
        sources = [mapping.source for mapping in self.mappings]
        targets = [mapping.target for mapping in self.mappings]
        default_targets = [target for target, _ in self.defaults]
        if len(set(sources)) != len(sources):
            raise ValueError("source columns must be unique")
        if len(set(targets + default_targets)) != len(targets) + len(default_targets):
            raise ValueError("canonical targets must be unique")
        if not self.provenance:
            raise ValueError("provenance is required")


_EXACT_SUFFIXES = ("_atoms", "_ns")
_EXACT_FIELDS = {"source_sequence", "canonical_tie_breaker", "trade_count"}
_REQUIRED_TOP_LEVEL = {
    "schema_version",
    "dataset_id",
    "instrument_id",
    "venue_id",
    "ts_event_ns",
    "canonical_tie_breaker",
    "kind",
    "payload",
    "quality_flags",
}


def _canonical_integer(value: object) -> str:
    if isinstance(value, bool | float):
        raise SchemaRejected("floating-point/boolean exact integer is forbidden")
    text = str(value)
    if text == "-0" or text.startswith("+") or not text:
        raise SchemaRejected("non-canonical exact integer")
    digits = text[1:] if text.startswith("-") else text
    if not digits.isascii() or not digits.isdigit():
        raise SchemaRejected("non-canonical exact integer")
    if len(digits) > 1 and digits.startswith("0"):
        raise SchemaRejected("non-canonical exact integer")
    return text


def _normalize_scalar(target: str, value: object) -> JsonValue:
    field = target.rsplit(".", maxsplit=1)[-1]
    if field.endswith(_EXACT_SUFFIXES) or field in _EXACT_FIELDS:
        return _canonical_integer(value)
    if isinstance(value, float):
        raise SchemaRejected("floating-point canonical values are forbidden")
    if value is None or isinstance(value, bool | int | str):
        return value
    raise SchemaRejected(f"unsupported source scalar for {target}: {type(value).__name__}")


def _assign(event: dict[str, JsonValue], target: str, value: JsonValue) -> None:
    if target.startswith("payload."):
        payload = event.setdefault("payload", {})
        if not isinstance(payload, dict):
            raise SchemaRejected("payload target conflicts with non-object default")
        payload[target.removeprefix("payload.")] = value
    else:
        event[target] = value


class CanonicalFileAdapter:
    """Imports one user-declared CSV or Parquet file inside a sandbox root."""

    def __init__(
        self,
        *,
        root: Path,
        source_path: Path,
        declaration: ImportDeclaration,
        limits: ImportLimits | None = None,
    ) -> None:
        self.root = root.resolve()
        self.source_path = self._resolve(source_path)
        self.declaration = declaration
        self.limits = ImportLimits() if limits is None else limits

    def _resolve(self, source_path: Path) -> Path:
        candidate = (
            (self.root / source_path).resolve()
            if not source_path.is_absolute()
            else source_path.resolve()
        )
        try:
            candidate.relative_to(self.root)
        except ValueError as error:
            raise PathRejected("source file escapes configured import root") from error
        if not candidate.is_file():
            raise PathRejected("source file does not exist or is not a regular file")
        return candidate

    def plan(self, request: FetchRequest) -> FetchPlan:
        """Return a one-file local plan after checking declared size."""
        del request
        size = self.source_path.stat().st_size
        if size > self.limits.max_file_bytes:
            raise ResourceLimitExceeded("source exceeds max_file_bytes")
        return FetchPlan(
            (FetchChunk("canonical-file", str(self.source_path), expected_bytes=size),)
        )

    def fetch(self, chunk: FetchChunk) -> bytes:
        """Read the declared file, rejecting archives and size races."""
        path = Path(chunk.source_ref).resolve()
        if path != self.source_path:
            raise PathRejected("fetch chunk does not reference the declared source")
        raw = path.read_bytes()
        if len(raw) > self.limits.max_file_bytes:
            raise ResourceLimitExceeded("source grew beyond max_file_bytes")
        if raw.startswith((b"PK\x03\x04", b"PK\x05\x06", b"PK\x07\x08")):
            raise ResourceLimitExceeded(
                "archive containers are not accepted by canonical-file import"
            )
        return raw

    def _rows_csv(self, raw: bytes) -> tuple[tuple[str, ...], list[dict[str, object]]]:
        try:
            text = raw.decode("utf-8", errors="strict")
        except UnicodeDecodeError as error:
            raise SchemaRejected("CSV must be UTF-8") from error
        if len(raw) > self.limits.max_materialized_bytes:
            raise ResourceLimitExceeded("CSV exceeds max_materialized_bytes")
        reader = csv.DictReader(io.StringIO(text, newline=""))
        if reader.fieldnames is None:
            raise SchemaRejected("CSV header is required")
        if len(reader.fieldnames) > self.limits.max_columns:
            raise ResourceLimitExceeded("CSV exceeds max_columns")
        rows: list[dict[str, object]] = []
        for row in reader:
            if len(rows) >= self.limits.max_rows:
                raise ResourceLimitExceeded("CSV exceeds max_rows")
            if None in row:
                raise SchemaRejected("CSV row has more values than declared header columns")
            rows.append({key: value for key, value in row.items()})
        return tuple(reader.fieldnames), rows

    def _rows_parquet(self, raw: bytes) -> tuple[tuple[str, ...], list[dict[str, object]]]:
        if not raw.startswith(b"PAR1") or not raw.endswith(b"PAR1"):
            raise SchemaRejected("invalid Parquet magic")
        parquet = pq.ParquetFile(io.BytesIO(raw))
        metadata = parquet.metadata
        if metadata.num_rows > self.limits.max_rows:
            raise ResourceLimitExceeded("Parquet exceeds max_rows")
        if metadata.num_columns > self.limits.max_columns:
            raise ResourceLimitExceeded("Parquet exceeds max_columns")
        declared_uncompressed = sum(
            metadata.row_group(row_group).column(column).total_uncompressed_size
            for row_group in range(metadata.num_row_groups)
            for column in range(metadata.num_columns)
        )
        if declared_uncompressed > self.limits.max_materialized_bytes:
            raise ResourceLimitExceeded("Parquet exceeds max_materialized_bytes")
        table = parquet.read(use_threads=False)
        if table.nbytes > self.limits.max_materialized_bytes:
            raise ResourceLimitExceeded("Parquet exceeds max_materialized_bytes")
        rows = [{str(key): value for key, value in row.items()} for row in table.to_pylist()]
        return tuple(str(name) for name in table.column_names), rows

    def normalize(self, chunk: FetchChunk, raw: bytes) -> NormalizedBatch:
        """Map source rows into canonical events with exact declared columns."""
        del chunk
        if self.declaration.format == "csv":
            columns, rows = self._rows_csv(raw)
        else:
            columns, rows = self._rows_parquet(raw)
        declared = tuple(mapping.source for mapping in self.declaration.mappings)
        if set(columns) != set(declared) or len(columns) != len(declared):
            raise SchemaRejected(
                f"source columns must exactly match declaration: {sorted(declared)!r}"
            )

        events: list[dict[str, JsonValue]] = []
        for row in rows:
            event: dict[str, JsonValue] = {}
            for target, default in self.declaration.defaults:
                _assign(event, target, default)
            for mapping in self.declaration.mappings:
                _assign(
                    event, mapping.target, _normalize_scalar(mapping.target, row[mapping.source])
                )
            if set(event) != _REQUIRED_TOP_LEVEL:
                missing = sorted(_REQUIRED_TOP_LEVEL - set(event))
                extra = sorted(set(event) - _REQUIRED_TOP_LEVEL)
                raise SchemaRejected(
                    f"canonical top-level fields mismatch; missing={missing}, extra={extra}"
                )
            events.append(event)
        return NormalizedBatch(tuple(events))


__all__ = [
    "CanonicalFileAdapter",
    "CanonicalFileError",
    "ColumnMapping",
    "ImportDeclaration",
    "ImportLimits",
    "PathRejected",
    "ResourceLimitExceeded",
    "SchemaRejected",
]
