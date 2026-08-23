"""Deterministic coverage derivation and setup eligibility."""

from __future__ import annotations

import hashlib
import json

from .model import (
    CoverageSegment,
    EligibleSetup,
    EligibleWindow,
    Gap,
    ManifestRecord,
    ManifestStatus,
    SetupRequirement,
)


class ManifestHashConflict(ValueError):
    """Same content identity was presented with different immutable facts."""


class UnknownManifest(KeyError):
    """Requested manifest is not present in this catalog."""


class CoverageCatalog:
    """Immutable-manifest catalog with separate append-only revocation facts."""

    def __init__(self, manifests: tuple[ManifestRecord, ...] = ()) -> None:
        self._manifests: dict[str, ManifestRecord] = {}
        self._revocations: dict[str, tuple[int, str]] = {}
        for manifest in manifests:
            self.register(manifest)

    def register(self, manifest: ManifestRecord) -> bool:
        """Register an immutable manifest; identical retries are idempotent.

        Returns `True` on first insert and `False` for an exact retry.

        # Raises
        `ManifestHashConflict` if the same manifest hash is associated with changed facts.
        """
        existing = self._manifests.get(manifest.manifest_hash)
        if existing is None:
            self._manifests[manifest.manifest_hash] = manifest
            return True
        if existing != manifest:
            raise ManifestHashConflict(manifest.manifest_hash)
        return False

    def revoke(self, manifest_hash: str, *, revoked_at_ns: int, reason: str) -> bool:
        """Append a revocation fact without rewriting the immutable manifest.

        Exact retries are idempotent. A changed revocation for the same hash is rejected.
        """
        if manifest_hash not in self._manifests:
            raise UnknownManifest(manifest_hash)
        if not reason:
            raise ValueError("revocation reason is required")
        if isinstance(revoked_at_ns, bool) or not -(2**63) <= revoked_at_ns <= 2**63 - 1:
            raise ValueError("revoked_at_ns must fit signed 64-bit integer")
        revocation = (revoked_at_ns, reason)
        existing = self._revocations.get(manifest_hash)
        if existing is None:
            self._revocations[manifest_hash] = revocation
            return True
        if existing != revocation:
            raise ValueError("manifest already has a different revocation fact")
        return False

    def is_revoked(self, manifest_hash: str) -> bool:
        """Return whether a manifest has a recorded revocation."""
        return manifest_hash in self._revocations

    def manifest_versions(self, manifest_id: str) -> tuple[ManifestRecord, ...]:
        """Return all immutable versions in deterministic ingestion/hash order."""
        return tuple(
            sorted(
                (
                    record
                    for record in self._manifests.values()
                    if record.manifest_id == manifest_id
                ),
                key=lambda record: (record.ingested_at_ns, record.manifest_hash),
            )
        )

    def segments(self, instrument_id: str | None = None) -> tuple[CoverageSegment, ...]:
        """Return all gap-free eligible-quality segments in deterministic order.

        Pending, quarantined, and revoked manifests never contribute coverage. Degraded
        manifests remain represented here so rulesets can explicitly allow or reject them.
        """
        segments: list[CoverageSegment] = []
        for manifest in self._manifests.values():
            if instrument_id is not None and manifest.instrument_id != instrument_id:
                continue
            if manifest.status not in {ManifestStatus.VALID, ManifestStatus.DEGRADED}:
                continue
            if self.is_revoked(manifest.manifest_hash):
                continue
            for start_ns, end_ns in _subtract_gaps(
                manifest.actual_start_ns,
                manifest.actual_end_ns,
                manifest.known_gaps,
            ):
                segments.append(
                    CoverageSegment(
                        instrument_id=manifest.instrument_id,
                        start_ns=start_ns,
                        end_ns=end_ns,
                        manifest_hash=manifest.manifest_hash,
                        provider=manifest.provider,
                        dataset=manifest.dataset,
                        venue_id=manifest.venue_id,
                        execution_tier=manifest.execution_tier,
                        capabilities=manifest.capabilities,
                        redistribution_class=manifest.redistribution_class,
                        status=manifest.status,
                    )
                )
        return tuple(sorted(segments))

    def eligible_setups(self, requirement: SetupRequirement) -> tuple[EligibleSetup, ...]:
        """Return manifests whose single gap-free segment covers the entire setup.

        Coverage is never stitched across a known gap or across different manifests.
        """
        result: list[EligibleSetup] = []
        for segment in self.segments(requirement.instrument_id):
            if not _segment_meets_policy(segment, requirement):
                continue
            if (
                segment.start_ns <= requirement.required_start_ns
                and segment.end_ns >= requirement.required_end_ns
            ):
                result.append(
                    EligibleSetup(
                        manifest_hash=segment.manifest_hash,
                        coverage_start_ns=segment.start_ns,
                        coverage_end_ns=segment.end_ns,
                        play_start_ns=requirement.play_start_ns,
                        play_end_ns=requirement.required_end_ns,
                    )
                )
        return tuple(sorted(result))

    def eligible_windows(self, requirement: SetupRequirement) -> tuple[EligibleWindow, ...]:
        """Return every legal play-start range for the requirement's warm-up/duration.

        `requirement.play_start_ns` is intentionally ignored for window discovery; all
        other policy fields are applied. Each returned window belongs to one gap-free
        immutable manifest segment.
        """
        result: list[EligibleWindow] = []
        for segment in self.segments(requirement.instrument_id):
            if not _segment_meets_policy(segment, requirement):
                continue
            earliest = segment.start_ns + requirement.warmup_ns
            latest = segment.end_ns - requirement.duration_ns
            if earliest <= latest:
                result.append(
                    EligibleWindow(
                        manifest_hash=segment.manifest_hash,
                        coverage_start_ns=segment.start_ns,
                        coverage_end_ns=segment.end_ns,
                        earliest_play_start_ns=earliest,
                        latest_play_start_ns=latest,
                    )
                )
        return tuple(sorted(result))

    def eligibility_hash(self, requirement: SetupRequirement) -> str:
        """Hash the exact deterministic eligible set for later commitment binding."""
        canonical = [
            {
                "manifest_hash": item.manifest_hash,
                "coverage_start_ns": str(item.coverage_start_ns),
                "coverage_end_ns": str(item.coverage_end_ns),
                "play_start_ns": str(item.play_start_ns),
                "play_end_ns": str(item.play_end_ns),
            }
            for item in self.eligible_setups(requirement)
        ]
        encoded = json.dumps(
            canonical,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=True,
        ).encode("ascii")
        return hashlib.sha256(encoded).hexdigest()


def _segment_meets_policy(segment: CoverageSegment, requirement: SetupRequirement) -> bool:
    if segment.execution_tier.rank < requirement.minimum_tier.rank:
        return False
    if not requirement.required_capabilities.issubset(segment.capabilities):
        return False
    if segment.redistribution_class not in requirement.allowed_redistribution:
        return False
    if segment.status == ManifestStatus.DEGRADED and not requirement.allow_degraded:
        return False
    return True


def _subtract_gaps(
    start_ns: int, end_ns: int, gaps: tuple[Gap, ...]
) -> tuple[tuple[int, int], ...]:
    clipped = sorted(
        (
            max(start_ns, gap.start_ns),
            min(end_ns, gap.end_ns),
        )
        for gap in gaps
        if gap.end_ns > start_ns and gap.start_ns < end_ns
    )
    merged: list[tuple[int, int]] = []
    for gap_start, gap_end in clipped:
        if gap_start >= gap_end:
            continue
        if merged and gap_start <= merged[-1][1]:
            merged[-1] = (merged[-1][0], max(merged[-1][1], gap_end))
        else:
            merged.append((gap_start, gap_end))

    cursor = start_ns
    result: list[tuple[int, int]] = []
    for gap_start, gap_end in merged:
        if cursor < gap_start:
            result.append((cursor, gap_start))
        cursor = max(cursor, gap_end)
    if cursor < end_ns:
        result.append((cursor, end_ns))
    return tuple(result)
