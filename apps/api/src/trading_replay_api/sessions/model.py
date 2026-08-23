"""Session setup, immutable pins, lifecycle states, and stable errors."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from dataclasses import dataclass
from enum import StrEnum
from typing import Self, cast

from trading_replay_api.catalog import DataCapability, ExecutionTier, RedistributionClass

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1
U64_MAX = 2**64 - 1


class SessionStatus(StrEnum):
    """Authoritative lifecycle state for one replay session."""

    SETUP = "SETUP"
    COMMITTED = "COMMITTED"
    RUNNING = "RUNNING"
    PAUSED = "PAUSED"
    COMPLETED = "COMPLETED"


class VisibilityMode(StrEnum):
    """Calendar-information policy committed before replay begins."""

    ABSOLUTE = "ABSOLUTE"
    RELATIVE = "RELATIVE"
    HIDDEN_CALENDAR = "HIDDEN_CALENDAR"


class SessionErrorCode(StrEnum):
    """Stable machine-readable lifecycle failure codes."""

    SESSION_EXISTS = "SESSION_EXISTS"
    SESSION_NOT_FOUND = "SESSION_NOT_FOUND"
    PRINCIPAL_MISMATCH = "PRINCIPAL_MISMATCH"
    VERSION_CONFLICT = "VERSION_CONFLICT"
    INVALID_TRANSITION = "INVALID_TRANSITION"
    SETUP_INELIGIBLE = "SETUP_INELIGIBLE"
    MANIFEST_INELIGIBLE = "MANIFEST_INELIGIBLE"
    RULESET_FIDELITY_UNSUPPORTED = "RULESET_FIDELITY_UNSUPPORTED"
    RULESET_CONFLICT = "RULESET_CONFLICT"
    ADVANCE_OUT_OF_RANGE = "ADVANCE_OUT_OF_RANGE"
    INVALID_FORK_TARGET = "INVALID_FORK_TARGET"
    INVALID_VALUE = "INVALID_VALUE"


class SessionLifecycleError(RuntimeError):
    """Lifecycle error carrying a stable code for API mapping."""

    def __init__(self, code: SessionErrorCode, message: str) -> None:
        super().__init__(message)
        self.code = code


@dataclass(frozen=True, slots=True)
class RulesetDefinition:
    """Canonical immutable ruleset definition and its content hash."""

    ruleset_id: str
    version: str
    allowed_execution_tiers: frozenset[ExecutionTier]
    canonical_body_json: str
    ruleset_hash: str

    @classmethod
    def from_body(
        cls,
        *,
        ruleset_id: str,
        version: str,
        allowed_execution_tiers: frozenset[ExecutionTier],
        body: Mapping[str, object],
    ) -> Self:
        """Create an immutable definition after canonical JSON validation."""
        if not ruleset_id:
            raise ValueError("ruleset_id is required")
        if not version:
            raise ValueError("ruleset version is required")
        if not allowed_execution_tiers:
            raise ValueError("allowed_execution_tiers cannot be empty")
        canonical_body_json = _canonical_json(body)
        identity = {
            "allowed_execution_tiers": sorted(item.value for item in allowed_execution_tiers),
            "body": cast(object, json.loads(canonical_body_json)),
            "ruleset_id": ruleset_id,
            "ruleset_version": version,
        }
        digest = hashlib.sha256(_canonical_json(identity).encode("utf-8")).hexdigest()
        return cls(
            ruleset_id=ruleset_id,
            version=version,
            allowed_execution_tiers=allowed_execution_tiers,
            canonical_body_json=canonical_body_json,
            ruleset_hash=digest,
        )

    def body(self) -> dict[str, object]:
        """Return a fresh JSON object suitable for database persistence."""
        decoded = json.loads(self.canonical_body_json)
        if not isinstance(decoded, dict):
            raise RuntimeError("canonical ruleset body is not an object")
        return cast(dict[str, object], decoded)


@dataclass(frozen=True, slots=True)
class SetupRequest:
    """Complete pre-commit setup request validated against catalog and ruleset."""

    instrument_id: str
    manifest_hash: str
    play_start_ns: int
    warmup_ns: int
    duration_ns: int
    execution_tier: ExecutionTier
    ruleset: RulesetDefinition
    visibility_mode: VisibilityMode = VisibilityMode.RELATIVE
    required_capabilities: frozenset[DataCapability] = frozenset()
    allowed_redistribution: frozenset[RedistributionClass] = frozenset(
        {
            RedistributionClass.REDISTRIBUTABLE,
            RedistributionClass.USER_LICENSED,
            RedistributionClass.RESTRICTED,
            RedistributionClass.UNKNOWN,
        }
    )
    allow_degraded: bool = False

    def __post_init__(self) -> None:
        if not self.instrument_id:
            raise ValueError("instrument_id is required")
        _validate_sha256(self.manifest_hash, "manifest_hash")
        _validate_i64(self.play_start_ns, "play_start_ns")
        if self.warmup_ns < 0:
            raise ValueError("warmup_ns cannot be negative")
        if self.duration_ns <= 0:
            raise ValueError("duration_ns must be positive")
        _validate_i64(self.play_start_ns - self.warmup_ns, "warm-up start")
        _validate_i64(self.play_start_ns + self.duration_ns, "play end")
        if not self.allowed_redistribution:
            raise ValueError("allowed_redistribution cannot be empty")

    @property
    def play_end_ns(self) -> int:
        """Exclusive logical end of the requested replay interval."""
        return self.play_start_ns + self.duration_ns


@dataclass(frozen=True, slots=True)
class CommittedSetup:
    """Immutable setup facts pinned by the first commit transition."""

    instrument_id: str
    manifest_hash: str
    eligibility_hash: str
    play_start_ns: int
    warmup_ns: int
    duration_ns: int
    execution_tier: ExecutionTier
    required_capabilities: frozenset[DataCapability]
    allowed_redistribution: frozenset[RedistributionClass]
    allow_degraded: bool
    visibility_mode: VisibilityMode
    ruleset_id: str
    ruleset_version: str
    ruleset_hash: str

    @property
    def play_end_ns(self) -> int:
        """Exclusive logical end of the committed replay interval."""
        return self.play_start_ns + self.duration_ns

    def to_payload(self) -> dict[str, object]:
        """Return canonical JSON-compatible event payload fields."""
        return {
            "allow_degraded": self.allow_degraded,
            "allowed_redistribution": sorted(item.value for item in self.allowed_redistribution),
            "duration_ns": str(self.duration_ns),
            "eligibility_hash": self.eligibility_hash,
            "execution_tier": self.execution_tier.value,
            "instrument_id": self.instrument_id,
            "manifest_hash": self.manifest_hash,
            "play_start_ns": str(self.play_start_ns),
            "required_capabilities": sorted(item.value for item in self.required_capabilities),
            "ruleset_hash": self.ruleset_hash,
            "ruleset_id": self.ruleset_id,
            "ruleset_version": self.ruleset_version,
            "visibility_mode": self.visibility_mode.value,
            "warmup_ns": str(self.warmup_ns),
        }

    @classmethod
    def from_payload(cls, payload: Mapping[str, object]) -> Self:
        """Decode a setup event payload without accepting unsafe numeric coercions."""
        return cls(
            instrument_id=_string_field(payload, "instrument_id"),
            manifest_hash=_sha_field(payload, "manifest_hash"),
            eligibility_hash=_sha_field(payload, "eligibility_hash"),
            play_start_ns=_decimal_i64_field(payload, "play_start_ns"),
            warmup_ns=_nonnegative_decimal_field(payload, "warmup_ns"),
            duration_ns=_positive_decimal_field(payload, "duration_ns"),
            execution_tier=ExecutionTier(_string_field(payload, "execution_tier")),
            required_capabilities=frozenset(
                DataCapability(value) for value in _string_list_field(payload, "required_capabilities")
            ),
            allowed_redistribution=frozenset(
                RedistributionClass(value)
                for value in _string_list_field(payload, "allowed_redistribution")
            ),
            allow_degraded=_bool_field(payload, "allow_degraded"),
            visibility_mode=VisibilityMode(_string_field(payload, "visibility_mode")),
            ruleset_id=_string_field(payload, "ruleset_id"),
            ruleset_version=_string_field(payload, "ruleset_version"),
            ruleset_hash=_sha_field(payload, "ruleset_hash"),
        )


@dataclass(frozen=True, slots=True)
class SessionRecord:
    """Materialized authoritative session state."""

    session_id: str
    principal_id: str
    status: SessionStatus
    version: int
    created_at_ns: int
    setup: CommittedSetup | None
    logical_time_ns: int | None
    parent_session_id: str | None = None


_TIER_CAPABILITIES: dict[ExecutionTier, frozenset[DataCapability]] = {
    ExecutionTier.F0: frozenset({DataCapability.BARS}),
    ExecutionTier.F0T: frozenset({DataCapability.TRADES}),
    ExecutionTier.F1: frozenset({DataCapability.TRADES, DataCapability.BBO}),
    ExecutionTier.F2: frozenset(
        {DataCapability.TRADES, DataCapability.L2_SNAPSHOTS, DataCapability.L2_DELTAS}
    ),
    ExecutionTier.F3: frozenset({DataCapability.L3}),
}


def capabilities_for_tier(tier: ExecutionTier) -> frozenset[DataCapability]:
    """Return the minimum data capabilities required by an execution tier."""
    return _TIER_CAPABILITIES[tier]


def validate_version(value: int) -> None:
    """Reject booleans, negatives, and values outside the uint64 contract."""
    if isinstance(value, bool) or value < 0 or value > U64_MAX:
        raise ValueError("session version must fit unsigned 64-bit integer")


def validate_i64(value: int, name: str) -> None:
    """Validate a signed 64-bit integer without bool coercion."""
    _validate_i64(value, name)


def _canonical_json(value: object) -> str:
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
        raise ValueError("ruleset body must be JSON-compatible") from error


def _reject_float(value: object) -> None:
    if isinstance(value, float):
        raise ValueError("floating-point values are not allowed in authoritative rulesets")
    if isinstance(value, Mapping):
        for key, child in value.items():
            if not isinstance(key, str):
                raise ValueError("ruleset object keys must be strings")
            _reject_float(child)
    elif isinstance(value, (list, tuple)):
        for child in value:
            _reject_float(child)


def _validate_sha256(value: str, name: str) -> None:
    if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
        raise ValueError(f"{name} must be lowercase SHA-256 hex")


def _validate_i64(value: int, name: str) -> None:
    if isinstance(value, bool) or value < I64_MIN or value > I64_MAX:
        raise ValueError(f"{name} must fit signed 64-bit integer")


def _string_field(payload: Mapping[str, object], key: str) -> str:
    value = payload.get(key)
    if not isinstance(value, str) or not value:
        raise ValueError(f"{key} must be a non-empty string")
    return value


def _sha_field(payload: Mapping[str, object], key: str) -> str:
    value = _string_field(payload, key)
    _validate_sha256(value, key)
    return value


def _decimal_i64_field(payload: Mapping[str, object], key: str) -> int:
    raw = _string_field(payload, key)
    if raw == "-0" or raw.startswith("+") or (raw.startswith("0") and raw != "0"):
        raise ValueError(f"{key} must use canonical decimal encoding")
    negative = raw.startswith("-")
    digits = raw[1:] if negative else raw
    if not digits.isdigit() or (negative and digits.startswith("0")):
        raise ValueError(f"{key} must use canonical decimal encoding")
    value = int(raw)
    _validate_i64(value, key)
    return value


def _nonnegative_decimal_field(payload: Mapping[str, object], key: str) -> int:
    value = _decimal_i64_field(payload, key)
    if value < 0:
        raise ValueError(f"{key} cannot be negative")
    return value


def _positive_decimal_field(payload: Mapping[str, object], key: str) -> int:
    value = _decimal_i64_field(payload, key)
    if value <= 0:
        raise ValueError(f"{key} must be positive")
    return value


def _string_list_field(payload: Mapping[str, object], key: str) -> tuple[str, ...]:
    value = payload.get(key)
    if not isinstance(value, list) or any(not isinstance(item, str) for item in value):
        raise ValueError(f"{key} must be a string list")
    return tuple(cast(list[str], value))


def _bool_field(payload: Mapping[str, object], key: str) -> bool:
    value = payload.get(key)
    if not isinstance(value, bool):
        raise ValueError(f"{key} must be boolean")
    return value
