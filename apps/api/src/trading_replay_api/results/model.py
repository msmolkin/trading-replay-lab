"""Exact evidence and immutable result-export models."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Protocol, cast

from trading_replay_api.commitments import EligibleEpisode
from trading_replay_api.sessions import SessionRecord

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1
U64_MAX = 2**64 - 1


class ResultErrorCode(StrEnum):
    """Stable result/export failure codes."""

    SESSION_UNAVAILABLE = "SESSION_UNAVAILABLE"
    SESSION_NOT_COMPLETED = "SESSION_NOT_COMPLETED"
    INVALID_EVIDENCE = "INVALID_EVIDENCE"
    PERSISTED_CONFLICT = "PERSISTED_CONFLICT"
    RESULT_CONFLICT = "RESULT_CONFLICT"
    RESULT_NOT_FOUND = "RESULT_NOT_FOUND"
    DATABASE_CONFLICT = "DATABASE_CONFLICT"


class ResultServiceError(RuntimeError):
    """Result failure carrying a stable API code."""

    def __init__(self, code: ResultErrorCode, message: str) -> None:
        super().__init__(message)
        self.code = code


class SessionReader(Protocol):
    """Principal-scoped session materialization boundary."""

    def get_session(self, *, session_id: str, principal_id: str) -> SessionRecord:
        """Return one owned session or fail without disclosing another principal."""
        ...


@dataclass(frozen=True, slots=True)
class CommandReplayMetadata:
    """Replay fields absent from the persisted M3-06 command row."""

    command_id: str
    logical_ts_ns: int
    arrival_seq: int

    def __post_init__(self) -> None:
        _identity(self.command_id, "command_id")
        _i64(self.logical_ts_ns, "logical_ts_ns")
        _u64(self.arrival_seq, "arrival_seq")


@dataclass(frozen=True, slots=True)
class CanonicalInput:
    """One exact simulator-kernel input required for offline reproduction."""

    session_id: str
    input_seq: int
    expected_state_version: int
    logical_ts_ns: int
    kind: str
    payload_hex: str

    def __post_init__(self) -> None:
        _identity(self.session_id, "session_id")
        _identity(self.kind, "kind")
        _u64(self.input_seq, "input_seq")
        _u64(self.expected_state_version, "expected_state_version")
        _i64(self.logical_ts_ns, "logical_ts_ns")
        _hex(self.payload_hex, "payload_hex", exact_bytes=None)


@dataclass(frozen=True, slots=True)
class StateHashEvidence:
    """One simulator-state commitment after an event."""

    event_seq: int
    hash: str

    def __post_init__(self) -> None:
        _u64(self.event_seq, "event_seq")
        _sha256(self.hash, "state hash")


@dataclass(frozen=True, slots=True)
class LedgerPostingEvidence:
    """One exact signed ledger posting."""

    account: str
    amount_minor: int
    currency: str

    def __post_init__(self) -> None:
        _identity(self.account, "account")
        _identity(self.currency, "currency")
        _i64(self.amount_minor, "amount_minor")


@dataclass(frozen=True, slots=True)
class LedgerTransactionEvidence:
    """One balanced transaction attached to a simulator event."""

    event_seq: int
    transaction_id: str
    postings: tuple[LedgerPostingEvidence, ...]

    def __post_init__(self) -> None:
        _u64(self.event_seq, "event_seq")
        _identity(self.transaction_id, "transaction_id")
        if len(self.postings) < 2:
            raise ValueError("ledger transaction requires at least two postings")


@dataclass(frozen=True, slots=True)
class ResultMetrics:
    """Exact final metrics used by the portable result commitment."""

    survived: bool
    terminal_return_ppb: int
    max_drawdown_ppb: int
    peak_effective_leverage_ppb: int
    benchmark_return_ppb: int

    def __post_init__(self) -> None:
        if not isinstance(self.survived, bool):
            raise ValueError("survived must be boolean")
        _i64(self.terminal_return_ppb, "terminal_return_ppb")
        _i64(self.max_drawdown_ppb, "max_drawdown_ppb")
        _i64(self.peak_effective_leverage_ppb, "peak_effective_leverage_ppb")
        _i64(self.benchmark_return_ppb, "benchmark_return_ppb")


@dataclass(frozen=True, slots=True)
class ResultEvidence:
    """Completion-only evidence not reconstructible from current persistence tables."""

    command_metadata: tuple[CommandReplayMetadata, ...]
    inputs: tuple[CanonicalInput, ...]
    state_hashes: tuple[StateHashEvidence, ...]
    ledger_transactions: tuple[LedgerTransactionEvidence, ...]
    metrics: ResultMetrics
    eligible_episodes: tuple[EligibleEpisode, ...] = ()


@dataclass(frozen=True, slots=True)
class FrozenResult:
    """One immutable canonical result/export package."""

    session_id: str
    result_hash: str
    bundle_hash: str
    proof_hash: str
    export_hash: str
    created_at_ns: int
    bundle: dict[str, object]
    proof: dict[str, object]
    export: dict[str, object]
    replayed: bool


def canonical_json(value: object) -> str:
    """Return deterministic JSON after rejecting floating point and unsafe keys."""
    _validate_json(value)
    return json.dumps(
        value,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=True,
        allow_nan=False,
    )


def canonical_hash(value: object) -> str:
    """SHA-256 hash of canonical UTF-8 JSON."""
    return hashlib.sha256(canonical_json(value).encode("utf-8")).hexdigest()


def mapping(value: object, name: str) -> Mapping[str, object]:
    """Require a JSON object with string keys."""
    if not isinstance(value, Mapping) or any(not isinstance(key, str) for key in value):
        raise ResultServiceError(ResultErrorCode.PERSISTED_CONFLICT, f"{name} is not an object")
    return cast(Mapping[str, object], value)


def _validate_json(value: object) -> None:
    if isinstance(value, float):
        raise ResultServiceError(
            ResultErrorCode.INVALID_EVIDENCE,
            "result evidence cannot contain floating-point values",
        )
    if value is None or isinstance(value, (str, int, bool)):
        return
    if isinstance(value, Mapping):
        for key, item in value.items():
            if not isinstance(key, str):
                raise ResultServiceError(
                    ResultErrorCode.INVALID_EVIDENCE,
                    "result object keys must be strings",
                )
            _validate_json(item)
        return
    if isinstance(value, (list, tuple)):
        for item in value:
            _validate_json(item)
        return
    raise ResultServiceError(
        ResultErrorCode.INVALID_EVIDENCE,
        "result evidence contains a non-JSON value",
    )


def _identity(value: str, name: str) -> None:
    if not value or len(value) > 200 or any(character in value for character in "\x00\r\n"):
        raise ValueError(f"invalid {name}")


def _i64(value: int, name: str) -> None:
    if isinstance(value, bool) or not isinstance(value, int) or value < I64_MIN or value > I64_MAX:
        raise ValueError(f"{name} must fit signed 64-bit integer")


def _u64(value: int, name: str) -> None:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0 or value > U64_MAX:
        raise ValueError(f"{name} must fit unsigned 64-bit integer")


def _sha256(value: str, name: str) -> None:
    if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
        raise ValueError(f"{name} must be lowercase SHA-256 hex")


def _hex(value: str, name: str, *, exact_bytes: int | None) -> None:
    if len(value) % 2 != 0 or any(character not in "0123456789abcdef" for character in value):
        raise ValueError(f"{name} must be lowercase even-length hex")
    if exact_bytes is not None and len(value) != exact_bytes * 2:
        raise ValueError(f"{name} must contain exactly {exact_bytes} bytes")


__all__ = [
    "CanonicalInput",
    "CommandReplayMetadata",
    "FrozenResult",
    "LedgerPostingEvidence",
    "LedgerTransactionEvidence",
    "ResultErrorCode",
    "ResultEvidence",
    "ResultMetrics",
    "ResultServiceError",
    "SessionReader",
    "StateHashEvidence",
    "canonical_hash",
    "canonical_json",
    "mapping",
]
