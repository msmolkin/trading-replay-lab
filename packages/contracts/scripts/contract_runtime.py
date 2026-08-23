"""Dependency-free wire validation shared by contract checks and generated Python models."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

INT64_MIN = -(2**63)
INT64_MAX = 2**63 - 1
UINT64_MAX = 2**64 - 1
JS_SAFE_MAX = 2**53 - 1
WIRE_SIGNED_SUFFIXES = ("_atoms", "_minor", "_ppb", "_ns")
WIRE_UNSIGNED_NAMES = {
    "arrival_seq", "event_seq", "source_sequence", "canonical_tie_breaker",
    "submitted_at_event_seq", "expected_session_version", "generation", "trade_count",
    "duplicates_removed", "timestamp_resolution_ns"
}


class ContractError(ValueError):
    pass


def _wire_int(value: Any, *, signed: bool, path: str) -> int:
    if not isinstance(value, str) or not value or (value.startswith("+") or value.strip() != value):
        raise ContractError(f"{path} must be a canonical base-10 string")
    if value == "-0" or (value.startswith("0") and value != "0") or value.startswith("-0"):
        raise ContractError(f"{path} is not canonical")
    try:
        parsed = int(value, 10)
    except ValueError as exc:
        raise ContractError(f"{path} is not an integer") from exc
    low = INT64_MIN if signed else 0
    high = INT64_MAX if signed else UINT64_MAX
    if not low <= parsed <= high:
        raise ContractError(f"{path} is outside {'int64' if signed else 'uint64'}")
    return parsed


def validate_document(value: Any, path: str = "$") -> None:
    if isinstance(value, float):
        raise ContractError(f"{path}: floating point is forbidden in canonical contracts")
    if isinstance(value, int) and not isinstance(value, bool) and abs(value) > JS_SAFE_MAX:
        raise ContractError(f"{path}: unsafe JSON integer must be a base-10 string")
    if isinstance(value, list):
        for index, item in enumerate(value):
            validate_document(item, f"{path}[{index}]")
        return
    if not isinstance(value, dict):
        return

    if "schema_version" in value and value["schema_version"] != "1.0.0":
        raise ContractError(f"{path}.schema_version: unsupported major/version")

    for key, item in value.items():
        child_path = f"{path}.{key}"
        if key in WIRE_UNSIGNED_NAMES:
            _wire_int(item, signed=False, path=child_path)
        elif key.endswith(WIRE_SIGNED_SUFFIXES):
            signed = not key.startswith("qty_") and not key.endswith("volume_atoms")
            _wire_int(item, signed=signed, path=child_path)
        validate_document(item, child_path)

    if value.get("command_type") == "ORDER":
        required = {
            "command_id", "session_id", "instrument_id", "side", "quantity_atoms", "order_type",
            "time_in_force", "reduce_only", "post_only", "marketable_only",
            "submitted_at_event_seq", "client_idempotency_key"
        }
        missing = required - value.keys()
        if missing:
            raise ContractError(f"{path}: missing order fields {sorted(missing)}")
        if value["post_only"] and value["marketable_only"]:
            raise ContractError(f"{path}: post_only and marketable_only are mutually exclusive")
        order_type = value["order_type"]
        if order_type == "MARKET" and (value["post_only"] or "limit_price_atoms" in value or "stop_price_atoms" in value):
            raise ContractError(f"{path}: invalid MARKET fields")
        if order_type == "LIMIT" and ("limit_price_atoms" not in value or "stop_price_atoms" in value):
            raise ContractError(f"{path}: invalid LIMIT fields")
        if order_type == "STOP_MARKET" and ("stop_price_atoms" not in value or "limit_price_atoms" in value or value["post_only"]):
            raise ContractError(f"{path}: invalid STOP_MARKET fields")
        if order_type == "STOP_LIMIT" and not {"stop_price_atoms", "limit_price_atoms"} <= value.keys():
            raise ContractError(f"{path}: invalid STOP_LIMIT fields")


def canonical_json(value: Any) -> str:
    validate_document(value)
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def load_and_validate(path: Path) -> Any:
    value = json.loads(path.read_text(encoding="utf-8"))
    validate_document(value)
    return value
