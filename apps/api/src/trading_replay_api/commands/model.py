"""Canonical trading-command request and response types."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Protocol, cast

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1
U64_MAX = 2**64 - 1


class CommandErrorCode(StrEnum):
    """Stable command API error codes."""

    INVALID_COMMAND = "INVALID_COMMAND"
    SESSION_NOT_FOUND = "SESSION_NOT_FOUND"
    PRINCIPAL_MISMATCH = "PRINCIPAL_MISMATCH"
    SESSION_NOT_RUNNING = "SESSION_NOT_RUNNING"
    VERSION_CONFLICT = "VERSION_CONFLICT"
    IDEMPOTENCY_CONFLICT = "IDEMPOTENCY_CONFLICT"
    COMMAND_NOT_FOUND = "COMMAND_NOT_FOUND"
    QUOTE_UNAVAILABLE = "QUOTE_UNAVAILABLE"
    DATABASE_CONFLICT = "DATABASE_CONFLICT"


class CommandServiceError(RuntimeError):
    """Command failure carrying a stable API code."""

    def __init__(self, code: CommandErrorCode, message: str) -> None:
        super().__init__(message)
        self.code = code


class CommandType(StrEnum):
    """Accepted authoritative command families."""

    SUBMIT_ORDER = "SUBMIT_ORDER"
    CANCEL_ORDER = "CANCEL_ORDER"
    REPLACE_ORDER = "REPLACE_ORDER"
    SET_LEVERAGE = "SET_LEVERAGE"


class PriceReference(StrEnum):
    """Visible quote shortcuts resolved by the server."""

    BID = "BID"
    ASK = "ASK"
    MIDPOINT = "MIDPOINT"


@dataclass(frozen=True, slots=True)
class VisibleQuote:
    """One already-visible quote used for command shortcut resolution."""

    event_id: str
    bid_price_atoms: int
    ask_price_atoms: int

    def __post_init__(self) -> None:
        if not self.event_id:
            raise ValueError("visible quote event_id is required")
        _validate_positive_i64(self.bid_price_atoms, "bid_price_atoms")
        _validate_positive_i64(self.ask_price_atoms, "ask_price_atoms")
        if self.bid_price_atoms >= self.ask_price_atoms:
            raise ValueError("visible quote must have bid below ask")


class VisibleQuoteResolver(Protocol):
    """Visibility-gated quote boundary supplied by M3-05."""

    def current_quote(self, *, session_id: str, principal_id: str) -> VisibleQuote | None:
        """Return only a quote visible at the session replay frontier."""
        ...


class Clock(Protocol):
    """Non-authoritative receipt timestamp boundary."""

    def now_ns(self) -> int:
        """Return current wall-clock nanoseconds for audit metadata only."""
        ...


@dataclass(frozen=True, slots=True)
class AcceptedCommand:
    """Persisted canonical command returned for new writes and exact retries."""

    command_id: str
    session_id: str
    idempotency_key: str
    payload_hash: str
    expected_session_version: int
    resulting_session_version: int
    accepted_at_ns: int
    payload: dict[str, object]
    replayed: bool


@dataclass(frozen=True, slots=True)
class PreparedCommand:
    """Server-canonical command before persistence."""

    payload: dict[str, object]
    payload_hash: str


def canonical_payload_hash(payload: Mapping[str, object]) -> str:
    """Hash deterministic JSON after rejecting floats and unsafe object keys."""
    return hashlib.sha256(canonical_json(payload).encode("utf-8")).hexdigest()


def canonical_json(value: object) -> str:
    """Return stable JSON without floats or non-string object keys."""
    _reject_float(value)
    try:
        return json.dumps(
            value,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=True,
            allow_nan=False,
        )
    except (TypeError, ValueError) as error:
        raise CommandServiceError(
            CommandErrorCode.INVALID_COMMAND,
            "command payload must be JSON-compatible",
        ) from error


def canonical_u64_text(value: object, name: str, *, positive: bool = False) -> str:
    """Validate canonical unsigned integer text without coercing JSON numbers."""
    if not isinstance(value, str) or not value:
        raise _invalid(f"{name} must be canonical unsigned decimal text")
    if value.startswith(("+", "-")) or (value.startswith("0") and value != "0"):
        raise _invalid(f"{name} must be canonical unsigned decimal text")
    if not value.isascii() or not value.isdigit():
        raise _invalid(f"{name} must be canonical unsigned decimal text")
    parsed = int(value)
    if parsed > U64_MAX or (positive and parsed == 0):
        raise _invalid(f"{name} is outside the allowed unsigned range")
    return value


def canonical_i64_text(value: object, name: str, *, positive: bool = False) -> str:
    """Validate canonical signed integer text without accepting floats."""
    if not isinstance(value, str) or not value or value.startswith("+") or value == "-0":
        raise _invalid(f"{name} must be canonical signed decimal text")
    digits = value[1:] if value.startswith("-") else value
    if (
        not digits
        or not digits.isascii()
        or not digits.isdigit()
        or (digits.startswith("0") and digits != "0")
    ):
        raise _invalid(f"{name} must be canonical signed decimal text")
    parsed = int(value)
    if parsed < I64_MIN or parsed > I64_MAX or (positive and parsed <= 0):
        raise _invalid(f"{name} is outside the allowed signed range")
    return value


def canonical_version(value: object) -> int:
    """Validate expected session version from API body/header conversion."""
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > U64_MAX:
        raise _invalid("expected_session_version must fit unsigned 64-bit integer")
    return value


def mapping(value: object, name: str) -> Mapping[str, object]:
    """Require a JSON object with string keys."""
    if not isinstance(value, Mapping):
        raise _invalid(f"{name} must be an object")
    if any(not isinstance(key, str) for key in value):
        raise _invalid(f"{name} keys must be strings")
    return cast(Mapping[str, object], value)


def require_string(fields: Mapping[str, object], name: str) -> str:
    """Read one required non-empty string."""
    value = fields.get(name)
    if not isinstance(value, str) or not value:
        raise _invalid(f"{name} must be a non-empty string")
    if any(character in value for character in "\x00\r\n"):
        raise _invalid(f"{name} contains forbidden control characters")
    return value


def optional_bool(fields: Mapping[str, object], name: str, *, default: bool = False) -> bool:
    """Read one strict boolean without truthy coercion."""
    value = fields.get(name, default)
    if not isinstance(value, bool):
        raise _invalid(f"{name} must be boolean")
    return value


def reject_unknown(fields: Mapping[str, object], allowed: frozenset[str]) -> None:
    """Reject unknown/client-authoritative fields instead of ignoring them."""
    unknown = sorted(set(fields).difference(allowed))
    if unknown:
        raise _invalid(f"unsupported command fields: {', '.join(unknown)}")


def _reject_float(value: object) -> None:
    if isinstance(value, float):
        raise _invalid("floating-point values are not allowed in authoritative commands")
    if isinstance(value, Mapping):
        for key, child in value.items():
            if not isinstance(key, str):
                raise _invalid("command object keys must be strings")
            _reject_float(child)
    elif isinstance(value, (list, tuple)):
        for child in value:
            _reject_float(child)


def _validate_positive_i64(value: int, name: str) -> None:
    if isinstance(value, bool) or value <= 0 or value > I64_MAX:
        raise ValueError(f"{name} must be a positive signed 64-bit integer")


def _invalid(message: str) -> CommandServiceError:
    return CommandServiceError(CommandErrorCode.INVALID_COMMAND, message)


__all__ = [
    "AcceptedCommand",
    "Clock",
    "CommandErrorCode",
    "CommandServiceError",
    "CommandType",
    "PreparedCommand",
    "PriceReference",
    "VisibleQuote",
    "VisibleQuoteResolver",
    "canonical_i64_text",
    "canonical_json",
    "canonical_payload_hash",
    "canonical_u64_text",
    "canonical_version",
    "mapping",
    "optional_bool",
    "reject_unknown",
    "require_string",
]
