from __future__ import annotations

from pathlib import Path

import pytest
from sqlalchemy import create_engine, inspect

from trading_replay_api.catalog import (
    CoverageCatalog,
    DataCapability,
    ExecutionTier,
    Gap,
    ManifestHashConflict,
    ManifestRecord,
    ManifestStatus,
    RedistributionClass,
    SetupRequirement,
    catalog_metadata,
)


def manifest(
    *,
    digest: str = "a" * 64,
    manifest_id: str = "dataset-v1",
    start: int = 0,
    end: int = 1_000,
    status: ManifestStatus = ManifestStatus.VALID,
    tier: ExecutionTier = ExecutionTier.F2,
    capabilities: frozenset[DataCapability] = frozenset(
        {DataCapability.TRADES, DataCapability.BBO, DataCapability.L2_SNAPSHOTS}
    ),
    gaps: tuple[Gap, ...] = (),
    redistribution: RedistributionClass = RedistributionClass.REDISTRIBUTABLE,
    ingested_at_ns: int = 1,
) -> ManifestRecord:
    return ManifestRecord(
        manifest_hash=digest,
        manifest_id=manifest_id,
        provider="recorded-provider",
        dataset="market-data",
        venue_id="TEST",
        instrument_id="SYNTH",
        adapter_version="1",
        canonical_content_hash="f" * 64,
        actual_start_ns=start,
        actual_end_ns=end,
        status=status,
        redistribution_class=redistribution,
        execution_tier=tier,
        capabilities=capabilities,
        known_gaps=gaps,
        quality_decisions=("validated",),
        provenance="fixture",
        ingested_at_ns=ingested_at_ns,
    )


def requirement(
    *,
    play_start: int = 200,
    warmup: int = 100,
    duration: int = 300,
    tier: ExecutionTier = ExecutionTier.F1,
    required: frozenset[DataCapability] = frozenset({DataCapability.TRADES}),
    allow_degraded: bool = False,
) -> SetupRequirement:
    return SetupRequirement(
        instrument_id="SYNTH",
        play_start_ns=play_start,
        warmup_ns=warmup,
        duration_ns=duration,
        minimum_tier=tier,
        required_capabilities=required,
        allow_degraded=allow_degraded,
    )


def test_overlapping_gaps_split_coverage_without_stitching() -> None:
    record = manifest(
        gaps=(
            Gap(300, 400, "gap-a"),
            Gap(350, 500, "overlap"),
            Gap(700, 750, "gap-b"),
        )
    )
    catalog = CoverageCatalog((record,))
    segments = catalog.segments("SYNTH")
    assert [(item.start_ns, item.end_ns) for item in segments] == [
        (0, 300),
        (500, 700),
        (750, 1_000),
    ]
    assert catalog.eligible_setups(requirement(play_start=550, warmup=100, duration=100)) == ()
    assert len(catalog.eligible_setups(requirement(play_start=600, warmup=50, duration=50))) == 1


def test_warmup_and_duration_must_fit_same_gap_free_segment() -> None:
    catalog = CoverageCatalog((manifest(gaps=(Gap(100, 120, "known"),)),))
    assert catalog.eligible_setups(requirement(play_start=200, warmup=100, duration=200)) == ()
    eligible = catalog.eligible_setups(requirement(play_start=200, warmup=80, duration=200))
    assert len(eligible) == 1
    assert eligible[0].coverage_start_ns == 120
    assert eligible[0].coverage_end_ns == 1_000


def test_window_discovery_applies_exact_warmup_and_duration() -> None:
    catalog = CoverageCatalog((manifest(start=100, end=1_000),))
    windows = catalog.eligible_windows(requirement(warmup=50, duration=200))
    assert len(windows) == 1
    assert windows[0].earliest_play_start_ns == 150
    assert windows[0].latest_play_start_ns == 800


def test_tier_capability_quality_and_license_filters_are_fail_closed() -> None:
    degraded = manifest(
        digest="b" * 64,
        status=ManifestStatus.DEGRADED,
        tier=ExecutionTier.F1,
        capabilities=frozenset({DataCapability.TRADES, DataCapability.BBO}),
        redistribution=RedistributionClass.USER_LICENSED,
    )
    catalog = CoverageCatalog((degraded,))
    assert catalog.eligible_setups(requirement()) == ()
    assert catalog.eligible_setups(requirement(allow_degraded=True, tier=ExecutionTier.F2)) == ()
    assert (
        catalog.eligible_setups(
            requirement(
                allow_degraded=True,
                tier=ExecutionTier.F1,
                required=frozenset({DataCapability.L2_DELTAS}),
            )
        )
        == ()
    )
    allowed = SetupRequirement(
        instrument_id="SYNTH",
        play_start_ns=200,
        warmup_ns=100,
        duration_ns=100,
        minimum_tier=ExecutionTier.F1,
        required_capabilities=frozenset({DataCapability.TRADES}),
        allowed_redistribution=frozenset({RedistributionClass.USER_LICENSED}),
        allow_degraded=True,
    )
    assert len(catalog.eligible_setups(allowed)) == 1


def test_revocation_is_separate_and_removes_eligibility() -> None:
    record = manifest()
    catalog = CoverageCatalog((record,))
    before = catalog.eligible_setups(requirement(duration=100))
    assert len(before) == 1
    assert catalog.revoke(record.manifest_hash, revoked_at_ns=99, reason="upstream withdrawn")
    assert catalog.is_revoked(record.manifest_hash)
    assert catalog.eligible_setups(requirement(duration=100)) == ()
    assert catalog.manifest_versions(record.manifest_id) == (record,)
    assert not catalog.revoke(record.manifest_hash, revoked_at_ns=99, reason="upstream withdrawn")
    with pytest.raises(ValueError, match="different revocation"):
        catalog.revoke(record.manifest_hash, revoked_at_ns=100, reason="changed")


def test_manifest_versions_and_hash_conflicts_are_deterministic() -> None:
    first = manifest(digest="1" * 64, ingested_at_ns=20)
    second = manifest(digest="2" * 64, ingested_at_ns=10)
    catalog = CoverageCatalog((first, second))
    assert catalog.manifest_versions("dataset-v1") == (second, first)
    assert not catalog.register(first)
    changed = manifest(digest="1" * 64, end=999, ingested_at_ns=20)
    with pytest.raises(ManifestHashConflict):
        catalog.register(changed)


def test_eligibility_hash_is_order_independent_and_sensitive_to_set() -> None:
    one = manifest(digest="1" * 64)
    two = manifest(digest="2" * 64, manifest_id="dataset-v2")
    query = requirement(duration=100)
    left = CoverageCatalog((one, two))
    right = CoverageCatalog((two, one))
    assert left.eligibility_hash(query) == right.eligibility_hash(query)
    right.revoke(two.manifest_hash, revoked_at_ns=5, reason="withdrawn")
    assert left.eligibility_hash(query) != right.eligibility_hash(query)


def test_invalid_time_math_fails_before_query() -> None:
    with pytest.raises(ValueError, match="warm-up start"):
        SetupRequirement(
            instrument_id="SYNTH",
            play_start_ns=-(2**63),
            warmup_ns=1,
            duration_ns=1,
            minimum_tier=ExecutionTier.F0,
        )
    with pytest.raises(ValueError, match="play end"):
        SetupRequirement(
            instrument_id="SYNTH",
            play_start_ns=2**63 - 1,
            warmup_ns=0,
            duration_ns=1,
            minimum_tier=ExecutionTier.F0,
        )


def test_catalog_schema_and_migration_pair_exist() -> None:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    catalog_metadata.create_all(engine)
    names = set(inspect(engine).get_table_names())
    assert {"catalog_manifests", "catalog_revocations"}.issubset(names)

    api_root = Path(__file__).resolve().parents[1]
    up = api_root / "migrations" / "0002_catalog.up.sql"
    down = api_root / "migrations" / "0002_catalog.down.sql"
    assert "CREATE TABLE catalog_manifests" in up.read_text(encoding="utf-8")
    assert "CREATE TABLE catalog_revocations" in up.read_text(encoding="utf-8")
    assert "DROP TABLE IF EXISTS catalog_manifests" in down.read_text(encoding="utf-8")
