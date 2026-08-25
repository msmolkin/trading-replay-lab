from __future__ import annotations

from dataclasses import dataclass, replace

import pytest
from fastapi.routing import APIRoute
from sqlalchemy import Engine, create_engine, insert, update

from trading_replay_api.auth import LocalAuthenticator
from trading_replay_api.catalog import ExecutionTier, RedistributionClass
from trading_replay_api.db.schema import metadata, result_bundles, rulesets, sessions
from trading_replay_api.results import (
    CanonicalInput,
    LedgerPostingEvidence,
    LedgerTransactionEvidence,
    ResultErrorCode,
    ResultEvidence,
    ResultMetrics,
    ResultService,
    ResultServiceError,
    ResultStore,
    StateHashEvidence,
    build_result_router,
    canonical_json,
)
from trading_replay_api.sessions import (
    CommittedSetup,
    SessionRecord,
    SessionStatus,
    VisibilityMode,
)


@dataclass(frozen=True, slots=True)
class FixedSessionReader:
    session: SessionRecord

    def get_session(self, *, session_id: str, principal_id: str) -> SessionRecord:
        assert session_id == self.session.session_id
        if principal_id != self.session.principal_id:
            raise ResultServiceError(ResultErrorCode.SESSION_UNAVAILABLE, "session unavailable")
        return self.session


def setup_record() -> CommittedSetup:
    return CommittedSetup(
        instrument_id="SYNTH",
        manifest_hash="11" * 32,
        eligibility_hash="22" * 32,
        play_start_ns=100,
        warmup_ns=0,
        duration_ns=10,
        execution_tier=ExecutionTier.F0,
        required_capabilities=frozenset(),
        allowed_redistribution=frozenset({RedistributionClass.REDISTRIBUTABLE}),
        allow_degraded=False,
        visibility_mode=VisibilityMode.RELATIVE,
        ruleset_id="rules-1",
        ruleset_version="1",
        ruleset_hash="33" * 32,
    )


def completed_session() -> SessionRecord:
    setup = setup_record()
    return SessionRecord(
        session_id="session-1",
        principal_id="principal-1",
        status=SessionStatus.COMPLETED,
        version=9,
        created_at_ns=0,
        setup=setup,
        logical_time_ns=setup.play_end_ns,
    )


def result_service(session: SessionRecord | None = None) -> tuple[ResultService, Engine]:
    current = completed_session() if session is None else session
    engine = create_engine("sqlite+pysqlite:///:memory:")
    metadata.create_all(engine)
    setup = setup_record()
    with engine.begin() as connection:
        connection.execute(
            insert(rulesets).values(
                ruleset_id=setup.ruleset_id,
                ruleset_version=setup.ruleset_version,
                ruleset_hash=setup.ruleset_hash,
                body_json={"fee_model": "fixed"},
            )
        )
        connection.execute(
            insert(sessions).values(
                session_id=current.session_id,
                principal_id=current.principal_id,
                status=current.status.value,
                version=current.version,
                ruleset_id=setup.ruleset_id,
                created_at_ns=current.created_at_ns,
            )
        )
    return (
        ResultService(
            sessions=FixedSessionReader(current),
            store=ResultStore(engine),
        ),
        engine,
    )


def evidence() -> ResultEvidence:
    return ResultEvidence(
        command_metadata=(),
        inputs=(
            CanonicalInput(
                session_id="session-1",
                input_seq=0,
                expected_state_version=0,
                logical_ts_ns=105,
                kind="TEST",
                payload_hex="00ff",
            ),
        ),
        state_hashes=(StateHashEvidence(event_seq=0, hash="44" * 32),),
        ledger_transactions=(
            LedgerTransactionEvidence(
                event_seq=0,
                transaction_id="tx-1",
                postings=(
                    LedgerPostingEvidence(account="CASH", amount_minor=-5, currency="USD"),
                    LedgerPostingEvidence(account="FEES", amount_minor=5, currency="USD"),
                ),
            ),
        ),
        metrics=ResultMetrics(
            survived=True,
            terminal_return_ppb=10,
            max_drawdown_ppb=-2,
            peak_effective_leverage_ppb=3,
            benchmark_return_ppb=4,
        ),
    )


def test_finalization_is_deterministic_write_once_and_offline_proof_is_complete() -> None:
    service, _ = result_service()
    first = service.finalize(
        session_id="session-1",
        principal_id="principal-1",
        evidence=evidence(),
        created_at_ns=123,
    )
    assert not first.replayed
    assert first.bundle["result_hash"] == first.result_hash
    assert first.proof["result_hash"] == first.result_hash
    assert first.export["bundle_hash"] == first.bundle_hash
    assert first.export["proof_hash"] == first.proof_hash
    events = first.proof["kernel_events"]
    assert isinstance(events, list)
    assert events[0]["prior_event_hash"] == "0" * 64
    assert (
        events[0]["payload_hash"]
        == "06eb7d6a69ee19e5fbdf749018d3d2abfa04bcbd1365db312eb86dc7169389b8"
    )

    retry = service.finalize(
        session_id="session-1",
        principal_id="principal-1",
        evidence=evidence(),
        created_at_ns=999,
    )
    assert retry.replayed
    assert retry.export_hash == first.export_hash
    assert retry.created_at_ns == 123


def test_changed_second_finalization_is_rejected() -> None:
    service, _ = result_service()
    service.finalize(
        session_id="session-1",
        principal_id="principal-1",
        evidence=evidence(),
        created_at_ns=123,
    )
    changed = replace(
        evidence(),
        metrics=replace(evidence().metrics, terminal_return_ppb=11),
    )
    with pytest.raises(ResultServiceError) as caught:
        service.finalize(
            session_id="session-1",
            principal_id="principal-1",
            evidence=changed,
            created_at_ns=124,
        )
    assert caught.value.code is ResultErrorCode.RESULT_CONFLICT


def test_store_detects_tampered_frozen_json() -> None:
    service, engine = result_service()
    service.finalize(
        session_id="session-1",
        principal_id="principal-1",
        evidence=evidence(),
        created_at_ns=123,
    )
    with engine.begin() as connection:
        connection.execute(
            update(result_bundles)
            .where(result_bundles.c.session_id == "session-1")
            .values(bundle_json={"result_hash": "00" * 32})
        )
    with pytest.raises(ResultServiceError) as caught:
        service.get(session_id="session-1", principal_id="principal-1")
    assert caught.value.code is ResultErrorCode.PERSISTED_CONFLICT


def test_unbalanced_ledger_and_noncompleted_session_fail_closed() -> None:
    service, _ = result_service()
    bad_ledger = replace(
        evidence(),
        ledger_transactions=(
            LedgerTransactionEvidence(
                event_seq=0,
                transaction_id="bad",
                postings=(
                    LedgerPostingEvidence(account="CASH", amount_minor=-5, currency="USD"),
                    LedgerPostingEvidence(account="FEES", amount_minor=4, currency="USD"),
                ),
            ),
        ),
    )
    with pytest.raises(ResultServiceError) as caught:
        service.finalize(
            session_id="session-1",
            principal_id="principal-1",
            evidence=bad_ledger,
            created_at_ns=123,
        )
    assert caught.value.code is ResultErrorCode.INVALID_EVIDENCE

    paused = replace(completed_session(), status=SessionStatus.PAUSED)
    paused_service, _ = result_service(paused)
    with pytest.raises(ResultServiceError) as not_completed:
        paused_service.finalize(
            session_id="session-1",
            principal_id="principal-1",
            evidence=evidence(),
            created_at_ns=123,
        )
    assert not_completed.value.code is ResultErrorCode.SESSION_NOT_COMPLETED


def test_canonical_json_rejects_float_and_router_is_read_only() -> None:
    with pytest.raises(ResultServiceError) as caught:
        canonical_json({"unsafe_money": 1.5})
    assert caught.value.code is ResultErrorCode.INVALID_EVIDENCE

    service, _ = result_service()
    router = build_result_router(
        service=service,
        authenticator=LocalAuthenticator(principal_id="principal-1"),
    )
    paths = {route.path for route in router.routes if isinstance(route, APIRoute)}
    assert paths == {
        "/sessions/{session_id}/result",
        "/sessions/{session_id}/result/proof",
        "/sessions/{session_id}/result/export",
    }
    assert all(
        "POST" not in route.methods
        for route in router.routes
        if isinstance(route, APIRoute)
    )
