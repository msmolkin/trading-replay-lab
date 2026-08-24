"""Deterministic replay-coordinator domain types and ports."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Protocol, cast

from trading_replay_api.sessions import SessionRecord

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1
U64_MAX = 2**64 - 1


class ReplayErrorCode(StrEnum):
    """Stable replay coordination failure codes."""

    SESSION_NOT_RUNNING = "SESSION_NOT_RUNNING"
    SESSION_NOT_COMMITTED = "SESSION_NOT_COMMITTED"
    TARGET_OUT_OF_RANGE = "TARGET_OUT_OF_RANGE"
    SOURCE_SEQUENCE = "SOURCE_SEQUENCE"
    SOURCE_TIME = "SOURCE_TIME"
    SIMULATOR_VERSION = "SIMULATOR_VERSION"
    SIMULATOR_HASH = "SIMULATOR_HASH"
    SNAPSHOT_CONFLICT = "SNAPSHOT_CONFLICT"
    SNAPSHOT_CORRUPT = "SNAPSHOT_CORRUPT"


class ReplayError(RuntimeError):
    """Replay failure carrying a stable API-safe classification."""

    def __init__(self, code: ReplayErrorCode, message: str) -> None:
        super().__init__(message)
        self.code = code


@dataclass(frozen=True, slots=True)
class ReplayInput:
    """One canonical market/scheduled input delivered to the simulator."""

    source_event_seq: int
    logical_ts_ns: int
    kind: str
    canonical_payload_json: str
    input_hash: str

    @classmethod
    def from_payload(
        cls,
        *,
        source_event_seq: int,
        logical_ts_ns: int,
        kind: str,
        payload: Mapping[str, object],
    ) -> ReplayInput:
        """Create an immutable input with deterministic canonical JSON and hash."""
        _validate_u64(source_event_seq, "source_event_seq")
        _validate_i64(logical_ts_ns, "logical_ts_ns")
        if not kind:
            raise ValueError("kind is required")
        canonical = canonical_json(payload)
        identity = canonical_json(
            {
                "kind": kind,
                "logical_ts_ns": str(logical_ts_ns),
                "payload": cast(object, json.loads(canonical)),
                "source_event_seq": str(source_event_seq),
            }
        )
        return cls(
            source_event_seq=source_event_seq,
            logical_ts_ns=logical_ts_ns,
            kind=kind,
            canonical_payload_json=canonical,
            input_hash=hashlib.sha256(identity.encode("utf-8")).hexdigest(),
        )

    def payload(self) -> dict[str, object]:
        """Return a fresh decoded payload object."""
        value = json.loads(self.canonical_payload_json)
        if not isinstance(value, dict):
            raise ReplayError(ReplayErrorCode.SNAPSHOT_CORRUPT, "input payload is not an object")
        return cast(dict[str, object], value)


@dataclass(frozen=True, slots=True)
class SimulatorState:
    """Verified continuation state returned by a simulator port."""

    state_version: int
    state_hash: str
    canonical_snapshot_json: str

    @classmethod
    def from_snapshot(
        cls,
        *,
        state_version: int,
        state_hash: str,
        snapshot: Mapping[str, object],
    ) -> SimulatorState:
        _validate_u64(state_version, "state_version")
        _validate_hash(state_hash, "state_hash")
        return cls(state_version, state_hash, canonical_json(snapshot))

    def snapshot(self) -> dict[str, object]:
        """Return a fresh decoded simulator snapshot object."""
        value = json.loads(self.canonical_snapshot_json)
        if not isinstance(value, dict):
            raise ReplayError(
                ReplayErrorCode.SNAPSHOT_CORRUPT,
                "simulator snapshot is not an object",
            )
        return cast(dict[str, object], value)


@dataclass(frozen=True, slots=True)
class ReplayCheckpoint:
    """Coordinator frontier plus the exact simulator continuation state."""

    logical_time_ns: int
    source_event_seq: int | None
    simulator: SimulatorState

    def __post_init__(self) -> None:
        _validate_i64(self.logical_time_ns, "logical_time_ns")
        if self.source_event_seq is not None:
            _validate_u64(self.source_event_seq, "source_event_seq")


@dataclass(frozen=True, slots=True)
class ReplayAdvanceResult:
    """Observable result of one coordinator advance/recovery."""

    session: SessionRecord
    checkpoint: ReplayCheckpoint
    applied_inputs: int
    recovered_inputs: int


@dataclass(frozen=True, slots=True)
class PersistedReplayEvent:
    """Persisted event projected for websocket/publication handoff."""

    event_seq: int
    logical_ts_ns: int
    event_type: str
    payload: dict[str, object]
    current_event_hash: str


class ReplaySource(Protocol):
    """Canonical ordered partition cursor boundary."""

    def next_after(
        self,
        *,
        manifest_hash: str,
        after_source_event_seq: int | None,
        through_ns: int,
        limit: int,
    ) -> tuple[ReplayInput, ...]:
        """Return the next canonical inputs in source-event order."""
        ...


class SimulatorPort(Protocol):
    """Stable boundary to the authoritative deterministic simulator."""

    def restore(self, checkpoint: ReplayCheckpoint | None) -> SimulatorState:
        """Restore the supplied checkpoint or return pristine simulator state."""
        ...

    def apply(
        self,
        input_event: ReplayInput,
        *,
        expected_state_version: int,
    ) -> SimulatorState:
        """Apply exactly one canonical input and return continuation state."""
        ...


class SessionLifecyclePort(Protocol):
    """Subset of session lifecycle used by replay coordination."""

    def get_session(self, *, session_id: str, principal_id: str) -> SessionRecord: ...

    def advance(
        self,
        *,
        session_id: str,
        principal_id: str,
        expected_version: int,
        logical_time_ns: int,
    ) -> SessionRecord: ...


def canonical_json(value: object) -> str:
    """Canonical JSON used for replay snapshots and input identities."""
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
        raise ValueError("value must be JSON-compatible") from error


def _reject_float(value: object) -> None:
    if isinstance(value, float):
        raise ValueError("floating-point values are not allowed in authoritative replay state")
    if isinstance(value, Mapping):
        for key, child in value.items():
            if not isinstance(key, str):
                raise ValueError("JSON object keys must be strings")
            _reject_float(child)
    elif isinstance(value, (list, tuple)):
        for child in value:
            _reject_float(child)


def _validate_u64(value: int, name: str) -> None:
    if isinstance(value, bool) or value < 0 or value > U64_MAX:
        raise ValueError(f"{name} must fit unsigned 64-bit integer")


def _validate_i64(value: int, name: str) -> None:
    if isinstance(value, bool) or value < I64_MIN or value > I64_MAX:
        raise ValueError(f"{name} must fit signed 64-bit integer")


def _validate_hash(value: str, name: str) -> None:
    if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
        raise ValueError(f"{name} must be lowercase SHA-256 hex")
