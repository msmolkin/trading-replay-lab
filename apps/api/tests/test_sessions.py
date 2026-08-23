from __future__ import annotations

from pathlib import Path

import pytest
from sqlalchemy import create_engine, select

from trading_replay_api.catalog import (
    CoverageCatalog,
    DataCapability,
    ExecutionTier,
    Gap,
    ManifestRecord,
    ManifestStatus,
    RedistributionClass,
)
from trading_replay_api.db.schema import domain_events
from trading_replay_api.db.store import ZERO_HASH
from trading_replay_api.sessions import (
    RulesetDefinition,
    SessionErrorCode,
    SessionLifecycleError,
    SessionService,
    SessionStatus,
    SetupRequest,
    VisibilityMode,
)


def manifest(
    *,
    digest: str = "a" * 64,
    gaps: tuple[Gap, ...] = (),
    capabilities: frozenset[DataCapability] = frozenset(
        {
            DataCapability.TRADES,
            DataCapability.BBO,
            DataCapability.L2_SNAPSHOTS,
            DataCapability.L2_DELTAS,
        }
    ),
) -> ManifestRecord:
    return ManifestRecord(
        manifest_hash=digest,
        manifest_id="session-dataset-v1",
        provider="recorded-provider",
        dataset="market-data",
        venue_id="TEST",
        instrument_id="SYNTH",
        adapter_version="1",
        canonical_content_hash="f" * 64,
        actual_start_ns=0,
        actual_end_ns=1_000,
        status=ManifestStatus.VALID,
        redistribution_class=RedistributionClass.REDISTRIBUTABLE,
        execution_tier=ExecutionTier.F2,
        capabilities=capabilities,
        known_gaps=gaps,
        provenance="fixture",
        ingested_at_ns=1,
    )


def ruleset(
    *,
    body: dict[str, object] | None = None,
    tiers: frozenset[ExecutionTier] = frozenset({ExecutionTier.F1, ExecutionTier.F2}),
) -> RulesetDefinition:
    return RulesetDefinition.from_body(
        ruleset_id="ruleset-1",
        version="1.0.0",
        allowed_execution_tiers=tiers,
        body=body or {"max_leverage": "5", "shorting": True},
    )


def setup(
    *,
    rules: RulesetDefinition | None = None,
    manifest_hash: str = "a" * 64,
    tier: ExecutionTier = ExecutionTier.F1,
) -> SetupRequest:
    return SetupRequest(
        instrument_id="SYNTH",
        manifest_hash=manifest_hash,
        play_start_ns=200,
        warmup_ns=100,
        duration_ns=300,
        execution_tier=tier,
        ruleset=rules or ruleset(),
        visibility_mode=VisibilityMode.HIDDEN_CALENDAR,
    )


def service(tmp_path: Path, *, catalog: CoverageCatalog | None = None) -> SessionService:
    tmp_path.mkdir(parents=True, exist_ok=True)
    engine = create_engine(f"sqlite+pysqlite:///{tmp_path / 'sessions.db'}")
    result = SessionService(engine, catalog or CoverageCatalog((manifest(),)))
    result.create_schema()
    return result


def created(api: SessionService, session_id: str = "session-1") -> None:
    record = api.create_session(
        session_id=session_id,
        principal_id="principal-1",
        created_at_ns=10,
    )
    assert record.status == SessionStatus.SETUP
    assert record.version == 0
    assert record.setup is None


def test_commit_pins_exact_manifest_ruleset_and_visibility(tmp_path: Path) -> None:
    api = service(tmp_path)
    created(api)
    committed = api.commit(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=0,
        setup=setup(),
    )
    assert committed.status == SessionStatus.COMMITTED
    assert committed.version == 1
    assert committed.logical_time_ns == 200
    assert committed.setup is not None
    assert committed.setup.manifest_hash == "a" * 64
    assert committed.setup.ruleset_id == "ruleset-1"
    assert len(committed.setup.ruleset_hash) == 64
    assert len(committed.setup.eligibility_hash) == 64
    assert committed.setup.visibility_mode == VisibilityMode.HIDDEN_CALENDAR
    assert committed.setup.required_capabilities == frozenset(
        {DataCapability.TRADES, DataCapability.BBO}
    )

    loaded = api.get_session(session_id="session-1", principal_id="principal-1")
    assert loaded == committed


def test_ruleset_fidelity_and_catalog_eligibility_fail_closed(tmp_path: Path) -> None:
    api = service(tmp_path)
    created(api)
    f1_only = ruleset(tiers=frozenset({ExecutionTier.F1}))
    with pytest.raises(SessionLifecycleError) as unsupported:
        api.commit(
            session_id="session-1",
            principal_id="principal-1",
            expected_version=0,
            setup=setup(rules=f1_only, tier=ExecutionTier.F2),
        )
    assert unsupported.value.code == SessionErrorCode.RULESET_FIDELITY_UNSUPPORTED

    gapped = service(
        tmp_path / "gapped",
        catalog=CoverageCatalog((manifest(gaps=(Gap(100, 250, "known-gap"),)),)),
    )
    created(gapped)
    with pytest.raises(SessionLifecycleError) as ineligible:
        gapped.commit(
            session_id="session-1",
            principal_id="principal-1",
            expected_version=0,
            setup=setup(),
        )
    assert ineligible.value.code == SessionErrorCode.SETUP_INELIGIBLE


def test_requested_manifest_must_be_member_of_eligible_set(tmp_path: Path) -> None:
    second = manifest(digest="b" * 64)
    api = service(tmp_path, catalog=CoverageCatalog((manifest(), second)))
    created(api)
    with pytest.raises(SessionLifecycleError) as rejected:
        api.commit(
            session_id="session-1",
            principal_id="principal-1",
            expected_version=0,
            setup=setup(manifest_hash="c" * 64),
        )
    assert rejected.value.code == SessionErrorCode.MANIFEST_INELIGIBLE


def test_lifecycle_is_monotonic_and_complete_only_at_committed_end(tmp_path: Path) -> None:
    api = service(tmp_path)
    created(api)
    record = api.commit(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=0,
        setup=setup(),
    )
    record = api.start(
        session_id="session-1", principal_id="principal-1", expected_version=record.version
    )
    assert record.status == SessionStatus.RUNNING
    record = api.advance(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=record.version,
        logical_time_ns=350,
    )
    assert record.logical_time_ns == 350
    record = api.pause(
        session_id="session-1", principal_id="principal-1", expected_version=record.version
    )
    assert record.status == SessionStatus.PAUSED
    record = api.start(
        session_id="session-1", principal_id="principal-1", expected_version=record.version
    )
    record = api.advance(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=record.version,
        logical_time_ns=500,
    )
    record = api.complete(
        session_id="session-1", principal_id="principal-1", expected_version=record.version
    )
    assert record.status == SessionStatus.COMPLETED
    assert record.logical_time_ns == 500
    assert record.version == 7

    with pytest.raises(SessionLifecycleError) as restart:
        api.start(
            session_id="session-1", principal_id="principal-1", expected_version=record.version
        )
    assert restart.value.code == SessionErrorCode.INVALID_TRANSITION


def test_invalid_advance_and_early_completion_are_atomic(tmp_path: Path) -> None:
    api = service(tmp_path)
    created(api)
    committed = api.commit(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=0,
        setup=setup(),
    )
    with pytest.raises(SessionLifecycleError) as early:
        api.complete(
            session_id="session-1",
            principal_id="principal-1",
            expected_version=committed.version,
        )
    assert early.value.code == SessionErrorCode.INVALID_TRANSITION

    running = api.start(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=committed.version,
    )
    for target in (200, 501):
        with pytest.raises(SessionLifecycleError) as invalid:
            api.advance(
                session_id="session-1",
                principal_id="principal-1",
                expected_version=running.version,
                logical_time_ns=target,
            )
        assert invalid.value.code == SessionErrorCode.ADVANCE_OUT_OF_RANGE
        assert (
            api.get_session(session_id="session-1", principal_id="principal-1").version
            == running.version
        )


def test_stale_version_and_cross_principal_access_fail_closed(tmp_path: Path) -> None:
    api = service(tmp_path)
    created(api)
    committed = api.commit(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=0,
        setup=setup(),
    )
    with pytest.raises(SessionLifecycleError) as stale:
        api.start(session_id="session-1", principal_id="principal-1", expected_version=0)
    assert stale.value.code == SessionErrorCode.VERSION_CONFLICT
    assert (
        api.get_session(session_id="session-1", principal_id="principal-1").version
        == committed.version
    )

    with pytest.raises(SessionLifecycleError) as isolated:
        api.get_session(session_id="session-1", principal_id="principal-2")
    assert isolated.value.code == SessionErrorCode.PRINCIPAL_MISMATCH


def test_fork_copies_immutable_pins_at_exact_frontier_without_mutating_parent(
    tmp_path: Path,
) -> None:
    api = service(tmp_path)
    created(api)
    parent = api.commit(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=0,
        setup=setup(),
    )
    parent = api.start(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=parent.version,
    )
    parent = api.advance(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=parent.version,
        logical_time_ns=325,
    )
    child = api.fork(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=parent.version,
        child_session_id="session-2",
        created_at_ns=20,
    )
    assert child.status == SessionStatus.PAUSED
    assert child.version == 1
    assert child.logical_time_ns == 325
    assert child.setup == parent.setup
    assert child.parent_session_id == "session-1"
    assert (
        api.get_session(session_id="session-1", principal_id="principal-1").version
        == parent.version
    )

    resumed = api.start(session_id="session-2", principal_id="principal-1", expected_version=1)
    assert resumed.status == SessionStatus.RUNNING


def test_ruleset_id_is_immutable_across_sessions(tmp_path: Path) -> None:
    api = service(tmp_path)
    created(api, "session-1")
    api.commit(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=0,
        setup=setup(),
    )
    created(api, "session-2")
    changed = ruleset(body={"max_leverage": "10", "shorting": True})
    with pytest.raises(SessionLifecycleError) as conflict:
        api.commit(
            session_id="session-2",
            principal_id="principal-1",
            expected_version=0,
            setup=setup(rules=changed),
        )
    assert conflict.value.code == SessionErrorCode.RULESET_CONFLICT
    assert (
        api.get_session(session_id="session-2", principal_id="principal-1").status
        == SessionStatus.SETUP
    )


def test_ruleset_rejects_binary_float_values() -> None:
    with pytest.raises(ValueError, match="floating-point"):
        ruleset(body={"fee_rate": 0.001})


def test_invalid_integer_inputs_use_stable_error_code(tmp_path: Path) -> None:
    api = service(tmp_path)
    with pytest.raises(SessionLifecycleError) as created_invalid:
        api.create_session(
            session_id="session-1",
            principal_id="principal-1",
            created_at_ns=True,
        )
    assert created_invalid.value.code == SessionErrorCode.INVALID_VALUE

    created(api)
    committed = api.commit(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=0,
        setup=setup(),
    )
    with pytest.raises(SessionLifecycleError) as version_invalid:
        api.start(
            session_id="session-1",
            principal_id="principal-1",
            expected_version=-1,
        )
    assert version_invalid.value.code == SessionErrorCode.INVALID_VALUE
    assert committed.version == 1


def test_lifecycle_events_form_a_persisted_hash_chain(tmp_path: Path) -> None:
    api = service(tmp_path)
    created(api)
    record = api.commit(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=0,
        setup=setup(),
    )
    api.start(session_id="session-1", principal_id="principal-1", expected_version=record.version)

    with api.engine.connect() as connection:
        rows = connection.execute(
            select(
                domain_events.c.event_seq,
                domain_events.c.prior_event_hash,
                domain_events.c.current_event_hash,
            )
            .where(domain_events.c.session_id == "session-1")
            .order_by(domain_events.c.event_seq)
        ).all()
    assert [int(row.event_seq) for row in rows] == [0, 1]
    assert rows[0].prior_event_hash == ZERO_HASH
    assert rows[1].prior_event_hash == rows[0].current_event_hash
    assert rows[0].current_event_hash != rows[1].current_event_hash
