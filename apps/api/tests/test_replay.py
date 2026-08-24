from __future__ import annotations

import hashlib
from dataclasses import replace

import pytest
from sqlalchemy import create_engine, insert

from trading_replay_api.catalog import DataCapability, ExecutionTier, RedistributionClass
from trading_replay_api.db.schema import metadata, sessions
from trading_replay_api.replay import (
    ReplayCheckpoint,
    ReplayCheckpointStore,
    ReplayCoordinator,
    ReplayError,
    ReplayErrorCode,
    ReplayInput,
    SimulatorState,
)
from trading_replay_api.sessions import CommittedSetup, SessionRecord, SessionStatus, VisibilityMode

ZERO_HASH = "0" * 64
MANIFEST_HASH = "a" * 64


class FakeLifecycle:
    def __init__(self, session: SessionRecord) -> None:
        self.session = session

    def get_session(self, *, session_id: str, principal_id: str) -> SessionRecord:
        assert session_id == self.session.session_id
        assert principal_id == self.session.principal_id
        return self.session

    def advance(
        self,
        *,
        session_id: str,
        principal_id: str,
        expected_version: int,
        logical_time_ns: int,
    ) -> SessionRecord:
        assert session_id == self.session.session_id
        assert principal_id == self.session.principal_id
        assert expected_version == self.session.version
        assert self.session.logical_time_ns is not None
        assert logical_time_ns > self.session.logical_time_ns
        self.session = replace(
            self.session,
            version=self.session.version + 1,
            logical_time_ns=logical_time_ns,
        )
        return self.session


class FakeSource:
    def __init__(self, events: tuple[ReplayInput, ...]) -> None:
        self.events = events

    def next_after(
        self,
        *,
        manifest_hash: str,
        after_source_event_seq: int | None,
        through_ns: int,
        limit: int,
    ) -> tuple[ReplayInput, ...]:
        assert manifest_hash == MANIFEST_HASH
        return tuple(
            event
            for event in self.events
            if (after_source_event_seq is None or event.source_event_seq > after_source_event_seq)
            and event.logical_ts_ns <= through_ns
        )[:limit]


class FakeSimulator:
    def __init__(self) -> None:
        self.state = SimulatorState.from_snapshot(
            state_version=0,
            state_hash=ZERO_HASH,
            snapshot={"state_version": "0"},
        )

    def restore(self, checkpoint: ReplayCheckpoint | None) -> SimulatorState:
        self.state = (
            SimulatorState.from_snapshot(
                state_version=0,
                state_hash=ZERO_HASH,
                snapshot={"state_version": "0"},
            )
            if checkpoint is None
            else checkpoint.simulator
        )
        return self.state

    def apply(
        self,
        input_event: ReplayInput,
        *,
        expected_state_version: int,
    ) -> SimulatorState:
        assert expected_state_version == self.state.state_version
        digest = hashlib.sha256(
            (self.state.state_hash + input_event.input_hash).encode("ascii")
        ).hexdigest()
        self.state = SimulatorState.from_snapshot(
            state_version=self.state.state_version + 1,
            state_hash=digest,
            snapshot={
                "last_source_event_seq": str(input_event.source_event_seq),
                "state_version": str(self.state.state_version + 1),
            },
        )
        return self.state


def setup() -> CommittedSetup:
    return CommittedSetup(
        instrument_id="SYNTH",
        manifest_hash=MANIFEST_HASH,
        eligibility_hash="b" * 64,
        play_start_ns=10,
        warmup_ns=10,
        duration_ns=20,
        execution_tier=ExecutionTier.F0,
        required_capabilities=frozenset({DataCapability.BARS}),
        allowed_redistribution=frozenset({RedistributionClass.REDISTRIBUTABLE}),
        allow_degraded=False,
        visibility_mode=VisibilityMode.RELATIVE,
        ruleset_id="rules-1",
        ruleset_version="1",
        ruleset_hash="c" * 64,
    )


def running_session(*, version: int = 2, logical_time_ns: int = 10) -> SessionRecord:
    return SessionRecord(
        session_id="session-1",
        principal_id="principal-1",
        status=SessionStatus.RUNNING,
        version=version,
        created_at_ns=0,
        setup=setup(),
        logical_time_ns=logical_time_ns,
    )


def events() -> tuple[ReplayInput, ...]:
    return (
        ReplayInput.from_payload(
            source_event_seq=0,
            logical_ts_ns=1,
            kind="BAR",
            payload={"close": "100"},
        ),
        ReplayInput.from_payload(
            source_event_seq=1,
            logical_ts_ns=10,
            kind="BAR",
            payload={"close": "101"},
        ),
        ReplayInput.from_payload(
            source_event_seq=2,
            logical_ts_ns=20,
            kind="BAR",
            payload={"close": "102"},
        ),
    )


def checkpoint_store() -> ReplayCheckpointStore:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    metadata.create_all(engine)
    with engine.begin() as connection:
        connection.execute(
            insert(sessions).values(
                session_id="session-1",
                principal_id="principal-1",
                status=SessionStatus.RUNNING.value,
                version=2,
                created_at_ns=0,
            )
        )
    return ReplayCheckpointStore(engine)


def run_with_batch_size(batch_size: int) -> tuple[ReplayCheckpoint, int, int]:
    lifecycle = FakeLifecycle(running_session())
    coordinator = ReplayCoordinator(
        lifecycle=lifecycle,
        source=FakeSource(events()),
        simulator=FakeSimulator(),
        checkpoints=checkpoint_store(),
    )
    result = coordinator.advance_to(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=2,
        target_logical_time_ns=20,
        batch_size=batch_size,
    )
    return result.checkpoint, result.applied_inputs, result.recovered_inputs


def test_batching_never_changes_canonical_result() -> None:
    one, applied_one, recovered_one = run_with_batch_size(1)
    many, applied_many, recovered_many = run_with_batch_size(100)

    assert one == many
    assert applied_one == applied_many == 1
    assert recovered_one == recovered_many == 2
    assert one.source_event_seq == 2
    assert one.simulator.state_version == 3


def test_missing_post_advance_checkpoint_is_recovered_exactly() -> None:
    store = checkpoint_store()
    simulator = FakeSimulator()
    source = FakeSource(events())
    state = simulator.restore(None)
    for event in events()[:2]:
        state = simulator.apply(event, expected_state_version=state.state_version)
    store.save(
        session_id="session-1",
        principal_id="principal-1",
        session_version=2,
        checkpoint=ReplayCheckpoint(10, 1, state),
    )

    lifecycle = FakeLifecycle(running_session(version=3, logical_time_ns=20))
    recovered = ReplayCoordinator(
        lifecycle=lifecycle,
        source=source,
        simulator=FakeSimulator(),
        checkpoints=store,
    ).advance_to(
        session_id="session-1",
        principal_id="principal-1",
        expected_version=3,
        target_logical_time_ns=20,
        batch_size=1,
    )

    uninterrupted, _, _ = run_with_batch_size(100)
    assert recovered.applied_inputs == 0
    assert recovered.recovered_inputs == 1
    assert recovered.checkpoint == uninterrupted
    assert store.load_latest(session_id="session-1", principal_id="principal-1") == uninterrupted


def test_source_sequence_gap_preserves_only_verified_frontier_checkpoint() -> None:
    gapped = (events()[0], events()[2])
    store = checkpoint_store()
    lifecycle = FakeLifecycle(running_session())
    coordinator = ReplayCoordinator(
        lifecycle=lifecycle,
        source=FakeSource(gapped),
        simulator=FakeSimulator(),
        checkpoints=store,
    )

    with pytest.raises(ReplayError) as caught:
        coordinator.advance_to(
            session_id="session-1",
            principal_id="principal-1",
            expected_version=2,
            target_logical_time_ns=20,
        )
    assert caught.value.code is ReplayErrorCode.SOURCE_SEQUENCE
    assert lifecycle.session.version == 2
    assert lifecycle.session.logical_time_ns == 10
    checkpoint = store.load_latest(session_id="session-1", principal_id="principal-1")
    assert checkpoint is not None
    assert checkpoint.logical_time_ns == 10
    assert checkpoint.source_event_seq == 0
    assert checkpoint.simulator.state_version == 1


def test_float_replay_payload_fails_closed() -> None:
    with pytest.raises(ValueError, match="floating-point"):
        ReplayInput.from_payload(
            source_event_seq=0,
            logical_ts_ns=1,
            kind="BAR",
            payload={"close": 100.5},
        )
