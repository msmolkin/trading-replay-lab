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
    "arrival_seq",
    "event_seq",
    "source_sequence",
    "canonical_tie_breaker",
    "submitted_at_event_seq",
    "expected_session_version",
    "generation",
    "trade_count",
    "duplicates_removed",
    "timestamp_resolution_ns",
}
CANONICAL_COMMAND_TYPES = {
    "SUBMIT_ORDER",
    "CANCEL_ORDER",
    "REPLACE_ORDER",
    "SET_LEVERAGE",
}
ORDER_TYPES = {"MARKET", "LIMIT", "STOP_MARKET", "STOP_LIMIT"}
TIME_IN_FORCE = {"GTC", "IOC", "FOK"}
PRICE_REFERENCES = {"BID", "ASK", "MIDPOINT"}


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


def _nonempty_text(value: Any, path: str) -> str:
    if not isinstance(value, str) or not value:
        raise ContractError(f"{path} must be a non-empty string")
    return value


def _boolean(value: Any, path: str) -> bool:
    if not isinstance(value, bool):
        raise ContractError(f"{path} must be boolean")
    return value


def _exact_keys(value: dict[str, Any], *, required: set[str], allowed: set[str], path: str) -> None:
    missing = required - value.keys()
    if missing:
        raise ContractError(f"{path}: missing fields {sorted(missing)}")
    unknown = value.keys() - allowed
    if unknown:
        raise ContractError(f"{path}: unknown fields {sorted(unknown)}")


def _positive_wire(value: Any, *, signed: bool, path: str) -> int:
    parsed = _wire_int(value, signed=signed, path=path)
    if parsed <= 0:
        raise ContractError(f"{path} must be positive")
    return parsed


def _validate_legacy_order(value: dict[str, Any], path: str) -> None:
    required = {
        "command_id",
        "session_id",
        "instrument_id",
        "side",
        "quantity_atoms",
        "order_type",
        "time_in_force",
        "reduce_only",
        "post_only",
        "marketable_only",
        "submitted_at_event_seq",
        "client_idempotency_key",
    }
    missing = required - value.keys()
    if missing:
        raise ContractError(f"{path}: missing order fields {sorted(missing)}")
    if value["post_only"] and value["marketable_only"]:
        raise ContractError(f"{path}: post_only and marketable_only are mutually exclusive")
    order_type = value["order_type"]
    if order_type == "MARKET" and (
        value["post_only"] or "limit_price_atoms" in value or "stop_price_atoms" in value
    ):
        raise ContractError(f"{path}: invalid MARKET fields")
    if order_type == "LIMIT" and (
        "limit_price_atoms" not in value or "stop_price_atoms" in value
    ):
        raise ContractError(f"{path}: invalid LIMIT fields")
    if order_type == "STOP_MARKET" and (
        "stop_price_atoms" not in value
        or "limit_price_atoms" in value
        or value["post_only"]
    ):
        raise ContractError(f"{path}: invalid STOP_MARKET fields")
    if order_type == "STOP_LIMIT" and not {
        "stop_price_atoms",
        "limit_price_atoms",
    } <= value.keys():
        raise ContractError(f"{path}: invalid STOP_LIMIT fields")


def _validate_submit(value: dict[str, Any], path: str) -> None:
    required = {
        "command_type",
        "instrument_id",
        "side",
        "quantity_atoms",
        "order_type",
        "time_in_force",
        "reduce_only",
        "post_only",
        "marketable_only",
    }
    allowed = required | {
        "limit_price_atoms",
        "stop_price_atoms",
        "price_reference",
        "quote_event_id",
    }
    _exact_keys(value, required=required, allowed=allowed, path=path)
    _nonempty_text(value["instrument_id"], f"{path}.instrument_id")
    if value["side"] not in {"BUY", "SELL"}:
        raise ContractError(f"{path}.side is invalid")
    _positive_wire(value["quantity_atoms"], signed=False, path=f"{path}.quantity_atoms")
    order_type = value["order_type"]
    if order_type not in ORDER_TYPES:
        raise ContractError(f"{path}.order_type is invalid")
    if value["time_in_force"] not in TIME_IN_FORCE:
        raise ContractError(f"{path}.time_in_force is invalid")
    post_only = _boolean(value["post_only"], f"{path}.post_only")
    marketable_only = _boolean(value["marketable_only"], f"{path}.marketable_only")
    _boolean(value["reduce_only"], f"{path}.reduce_only")
    if post_only and marketable_only:
        raise ContractError(f"{path}: post_only and marketable_only are mutually exclusive")
    for field in ("limit_price_atoms", "stop_price_atoms"):
        if field in value:
            _positive_wire(value[field], signed=True, path=f"{path}.{field}")

    if order_type == "MARKET":
        if post_only or any(
            field in value
            for field in ("limit_price_atoms", "stop_price_atoms", "price_reference", "quote_event_id")
        ):
            raise ContractError(f"{path}: invalid MARKET fields")
    elif order_type == "LIMIT":
        if "limit_price_atoms" not in value or "stop_price_atoms" in value:
            raise ContractError(f"{path}: invalid LIMIT fields")
    elif order_type == "STOP_MARKET":
        if (
            "stop_price_atoms" not in value
            or "limit_price_atoms" in value
            or "price_reference" in value
            or "quote_event_id" in value
            or post_only
        ):
            raise ContractError(f"{path}: invalid STOP_MARKET fields")
    elif not {"stop_price_atoms", "limit_price_atoms"} <= value.keys():
        raise ContractError(f"{path}: invalid STOP_LIMIT fields")

    has_reference = "price_reference" in value
    has_quote = "quote_event_id" in value
    if has_reference:
        if value["price_reference"] not in PRICE_REFERENCES:
            raise ContractError(f"{path}.price_reference is invalid")
        if not has_quote or "limit_price_atoms" not in value:
            raise ContractError(f"{path}: resolved price reference is incomplete")
    if has_quote:
        _nonempty_text(value["quote_event_id"], f"{path}.quote_event_id")
        if not has_reference:
            raise ContractError(f"{path}: quote_event_id requires price_reference")


def _validate_cancel(value: dict[str, Any], path: str) -> None:
    required = {"command_type", "order_id"}
    _exact_keys(value, required=required, allowed=required, path=path)
    _nonempty_text(value["order_id"], f"{path}.order_id")


def _validate_replace(value: dict[str, Any], path: str) -> None:
    required = {"command_type", "order_id"}
    mutations = {
        "quantity_atoms",
        "limit_price_atoms",
        "stop_price_atoms",
        "time_in_force",
        "reduce_only",
        "post_only",
        "marketable_only",
    }
    _exact_keys(value, required=required, allowed=required | mutations, path=path)
    _nonempty_text(value["order_id"], f"{path}.order_id")
    if not mutations.intersection(value):
        raise ContractError(f"{path}: replacement has no mutation fields")
    if "quantity_atoms" in value:
        _positive_wire(value["quantity_atoms"], signed=False, path=f"{path}.quantity_atoms")
    for field in ("limit_price_atoms", "stop_price_atoms"):
        if field in value:
            _positive_wire(value[field], signed=True, path=f"{path}.{field}")
    if "time_in_force" in value and value["time_in_force"] not in TIME_IN_FORCE:
        raise ContractError(f"{path}.time_in_force is invalid")
    for field in ("reduce_only", "post_only", "marketable_only"):
        if field in value:
            _boolean(value[field], f"{path}.{field}")
    if value.get("post_only") is True and value.get("marketable_only") is True:
        raise ContractError(f"{path}: post_only and marketable_only are mutually exclusive")


def _validate_set_leverage(value: dict[str, Any], path: str) -> None:
    required = {"command_type", "leverage"}
    _exact_keys(value, required=required, allowed=required, path=path)
    leverage = value["leverage"]
    if isinstance(leverage, bool) or not isinstance(leverage, int) or not 1 <= leverage <= 50:
        raise ContractError(f"{path}.leverage must be an integer from 1 through 50")


def _validate_canonical_command(value: Any, path: str) -> None:
    if not isinstance(value, dict):
        raise ContractError(f"{path} must be a command object")
    command_type = value.get("command_type")
    if command_type == "SUBMIT_ORDER":
        _validate_submit(value, path)
    elif command_type == "CANCEL_ORDER":
        _validate_cancel(value, path)
    elif command_type == "REPLACE_ORDER":
        _validate_replace(value, path)
    elif command_type == "SET_LEVERAGE":
        _validate_set_leverage(value, path)
    else:
        raise ContractError(f"{path}.command_type is not a canonical command type")


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
        _validate_legacy_order(value, path)
    elif value.get("command_type") in CANONICAL_COMMAND_TYPES:
        _validate_canonical_command(value, path)

    envelope_fields = {
        "schema_version",
        "command_id",
        "idempotency_key",
        "session_id",
        "principal_id",
        "accepted_at_ns",
        "logical_ts_ns",
        "arrival_seq",
        "expected_session_version",
        "payload",
        "payload_hash",
    }
    if envelope_fields <= value.keys():
        _validate_canonical_command(value["payload"], f"{path}.payload")


def canonical_json(value: Any) -> str:
    validate_document(value)
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def load_and_validate(path: Path) -> Any:
    value = json.loads(path.read_text(encoding="utf-8"))
    validate_document(value)
    return value
