"""Deterministic canonical JSONL writer used before Parquet task ownership lands."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

from .canonical import JsonValue, canonical_bytes, require_json_value


def _event_sort_key(event: dict[str, JsonValue]) -> tuple[int, int, int]:
    timestamp = event.get("ts_event_ns")
    sequence = event.get("source_sequence", "0")
    tie = event.get("canonical_tie_breaker", "0")
    if not isinstance(timestamp, str):
        raise ValueError("event is missing string ts_event_ns")
    if not isinstance(sequence, str) or not isinstance(tie, str):
        raise ValueError("event sequence fields must be strings")
    return int(timestamp), int(sequence), int(tie)


def write_canonical(
    output_path: Path,
    manifest_path: Path,
    *,
    job_id: str,
    events: list[dict[str, JsonValue]],
    chunk_hashes: list[tuple[str, str]],
) -> str:
    """Write ordered canonical events and a deterministic provenance manifest."""
    normalized_events: list[dict[str, JsonValue]] = []
    for event in events:
        normalized = require_json_value(event)
        if not isinstance(normalized, dict):
            raise TypeError("canonical market event must be an object")
        normalized_events.append(normalized)
    normalized_events.sort(key=_event_sort_key)
    body = b"".join(canonical_bytes(event) + b"\n" for event in normalized_events)
    content_hash = hashlib.sha256(body).hexdigest()

    output_path.parent.mkdir(parents=True, exist_ok=True)
    temporary = output_path.with_suffix(output_path.suffix + ".tmp")
    temporary.write_bytes(body)
    temporary.replace(output_path)

    manifest = {
        "canonical_content_sha256": content_hash,
        "chunks": [[key, digest] for key, digest in chunk_hashes],
        "job_id": job_id,
        "row_count": len(normalized_events),
    }
    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    manifest_temporary = manifest_path.with_suffix(manifest_path.suffix + ".tmp")
    manifest_temporary.write_text(
        json.dumps(manifest, sort_keys=True, indent=2) + "\n",
        encoding="utf-8",
        newline="\n",
    )
    manifest_temporary.replace(manifest_path)
    return content_hash
