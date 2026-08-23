"""Immutable catalog facts and deterministic setup-query types."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import StrEnum

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1


class ManifestStatus(StrEnum):
    """Manifest quality status relevant to setup eligibility."""

    PENDING = "PENDING"
    VALID = "VALID"
    DEGRADED = "DEGRADED"
    QUARANTINED = "QUARANTINED"


class RedistributionClass(StrEnum):
    """Dataset redistribution/licensing class from the v1 manifest."""

    REDISTRIBUTABLE = "REDISTRIBUTABLE"
    USER_LICENSED = "USER_LICENSED"
    RESTRICTED = "RESTRICTED"
    UNKNOWN = "UNKNOWN"


class ExecutionTier(StrEnum):
    """Execution fidelity provided by one manifest."""

    F0 = "F0"
    F0T = "F0T"
    F1 = "F1"
    F2 = "F2"
    F3 = "F3"

    @property
    def rank(self) -> int:
        """Return the monotonic fidelity rank used for minimum-tier queries."""
        return {
            ExecutionTier.F0: 0,
            ExecutionTier.F0T: 1,
            ExecutionTier.F1: 2,
            ExecutionTier.F2: 3,
            ExecutionTier.F3: 4,
        }[self]


class DataCapability(StrEnum):
    """Normalized data capabilities used by ruleset/setup filtering."""

    BARS = "BARS"
    TRADES = "TRADES"
    BBO = "BBO"
    L2_SNAPSHOTS = "L2_SNAPSHOTS"
    L2_DELTAS = "L2_DELTAS"
    L3 = "L3"
    MARK_PRICE = "MARK_PRICE"
    INDEX_PRICE = "INDEX_PRICE"
    FUNDING = "FUNDING"
    OPEN_INTEREST = "OPEN_INTEREST"
    LIQUIDATIONS = "LIQUIDATIONS"


@dataclass(frozen=True, slots=True, order=True)
class Gap:
    """Known unavailable half-open interval."""

    start_ns: int
    end_ns: int
    reason: str

    def __post_init__(self) -> None:
        _validate_i64(self.start_ns, "gap start_ns")
        _validate_i64(self.end_ns, "gap end_ns")
        if self.end_ns <= self.start_ns:
            raise ValueError("gap end_ns must be greater than start_ns")
        if not self.reason:
            raise ValueError("gap reason is required")


@dataclass(frozen=True, slots=True)
class ManifestRecord:
    """Immutable catalog projection of one validated dataset manifest."""

    manifest_hash: str
    manifest_id: str
    provider: str
    dataset: str
    venue_id: str
    instrument_id: str
    adapter_version: str
    canonical_content_hash: str
    actual_start_ns: int
    actual_end_ns: int
    status: ManifestStatus
    redistribution_class: RedistributionClass
    execution_tier: ExecutionTier
    capabilities: frozenset[DataCapability]
    known_gaps: tuple[Gap, ...] = ()
    quality_decisions: tuple[str, ...] = ()
    provenance: str = ""
    ingested_at_ns: int = 0

    def __post_init__(self) -> None:
        _validate_sha256(self.manifest_hash, "manifest_hash")
        _validate_sha256(self.canonical_content_hash, "canonical_content_hash")
        _validate_i64(self.actual_start_ns, "actual_start_ns")
        _validate_i64(self.actual_end_ns, "actual_end_ns")
        _validate_i64(self.ingested_at_ns, "ingested_at_ns")
        if self.actual_end_ns <= self.actual_start_ns:
            raise ValueError("manifest actual_end_ns must be greater than actual_start_ns")
        for value, name in (
            (self.manifest_id, "manifest_id"),
            (self.provider, "provider"),
            (self.dataset, "dataset"),
            (self.venue_id, "venue_id"),
            (self.instrument_id, "instrument_id"),
            (self.adapter_version, "adapter_version"),
        ):
            if not value:
                raise ValueError(f"{name} is required")


@dataclass(frozen=True, slots=True, order=True)
class CoverageSegment:
    """Gap-free coverage from one immutable manifest."""

    instrument_id: str
    start_ns: int
    end_ns: int
    manifest_hash: str
    provider: str
    dataset: str
    venue_id: str
    execution_tier: ExecutionTier = field(compare=False)
    capabilities: frozenset[DataCapability] = field(compare=False)
    redistribution_class: RedistributionClass = field(compare=False)
    status: ManifestStatus = field(compare=False)


@dataclass(frozen=True, slots=True)
class SetupRequirement:
    """Exact setup request whose complete visibility interval must be covered."""

    instrument_id: str
    play_start_ns: int
    warmup_ns: int
    duration_ns: int
    minimum_tier: ExecutionTier
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
        _validate_i64(self.play_start_ns, "play_start_ns")
        if self.warmup_ns < 0:
            raise ValueError("warmup_ns cannot be negative")
        if self.duration_ns <= 0:
            raise ValueError("duration_ns must be positive")
        _checked_i64(self.play_start_ns - self.warmup_ns, "warm-up start")
        _checked_i64(self.play_start_ns + self.duration_ns, "play end")
        if not self.allowed_redistribution:
            raise ValueError("allowed_redistribution cannot be empty")

    @property
    def required_start_ns(self) -> int:
        """Coverage frontier required before visible play begins."""
        return self.play_start_ns - self.warmup_ns

    @property
    def required_end_ns(self) -> int:
        """Exclusive coverage end required for the requested play duration."""
        return self.play_start_ns + self.duration_ns


@dataclass(frozen=True, slots=True, order=True)
class EligibleSetup:
    """One manifest/segment that exactly satisfies a setup requirement."""

    manifest_hash: str
    coverage_start_ns: int
    coverage_end_ns: int
    play_start_ns: int
    play_end_ns: int


@dataclass(frozen=True, slots=True, order=True)
class EligibleWindow:
    """Range of legal play-start timestamps within one gap-free segment."""

    manifest_hash: str
    coverage_start_ns: int
    coverage_end_ns: int
    earliest_play_start_ns: int
    latest_play_start_ns: int


def _validate_sha256(value: str, name: str) -> None:
    if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
        raise ValueError(f"{name} must be lowercase SHA-256 hex")


def _validate_i64(value: int, name: str) -> None:
    if isinstance(value, bool) or value < I64_MIN or value > I64_MAX:
        raise ValueError(f"{name} must fit signed 64-bit integer")


def _checked_i64(value: int, name: str) -> int:
    _validate_i64(value, name)
    return value
