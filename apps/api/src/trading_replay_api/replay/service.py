"""Deterministic replay coordinator with crash-safe checkpoint recovery."""

from __future__ import annotations

from dataclasses import dataclass

from trading_replay_api.sessions import SessionRecord, SessionStatus

from .model import (
    ReplayAdvanceResult,
    ReplayCheckpoint,
    ReplayError,
    ReplayErrorCode,
    ReplayInput,
    ReplaySource,
    SessionLifecyclePort,
    SimulatorPort,
    SimulatorState,
)
from .store import ReplayCheckpointStore


@dataclass(frozen=True, slots=True)
class _RunResult:
    checkpoint: ReplayCheckpoint
    applied_inputs: int


class ReplayCoordinator:
    """Advance one committed session through canonical inputs deterministically."""

    def __init__(
        self,
        *,
        lifecycle: SessionLifecyclePort,
        source: ReplaySource,
        simulator: SimulatorPort,
        checkpoints: ReplayCheckpointStore,
    ) -> None:
        self.lifecycle = lifecycle
        self.source = source
        self.simulator = simulator
        self.checkpoints = checkpoints

    def advance_to(
        self,
        *,
        session_id: str,
        principal_id: str,
        expected_version: int,
        target_logical_time_ns: int,
        batch_size: int = 512,
    ) -> ReplayAdvanceResult:
        """Recover the simulator frontier, then atomically advance session logical time.

        Session lifecycle persistence is authoritative. A crash after lifecycle advancement but
        before checkpoint persistence is recovered by deterministically replaying from the latest
        older checkpoint to the already-persisted logical frontier before accepting more input.
        """
        if isinstance(batch_size, bool) or batch_size <= 0 or batch_size > 100_000:
            raise ValueError("batch_size must be between 1 and 100000")
        session = self.lifecycle.get_session(session_id=session_id, principal_id=principal_id)
        if session.version != expected_version:
            raise ReplayError(
                ReplayErrorCode.TARGET_OUT_OF_RANGE,
                f"expected session version {expected_version}, stored {session.version}",
            )
        _require_running(session)
        assert session.setup is not None
        assert session.logical_time_ns is not None
        if (
            target_logical_time_ns < session.logical_time_ns
            or target_logical_time_ns > session.setup.play_end_ns
        ):
            raise ReplayError(
                ReplayErrorCode.TARGET_OUT_OF_RANGE,
                "target must not move backward or cross the committed replay end",
            )

        checkpoint = self.checkpoints.load_latest(
            session_id=session_id,
            principal_id=principal_id,
        )
        recovered = self._recover_current_frontier(
            session=session,
            checkpoint=checkpoint,
            batch_size=batch_size,
        )
        checkpoint = recovered.checkpoint
        if checkpoint.logical_time_ns != session.logical_time_ns:
            raise ReplayError(
                ReplayErrorCode.SNAPSHOT_CORRUPT,
                "recovery did not land on the persisted logical frontier",
            )
        self.checkpoints.save(
            session_id=session_id,
            principal_id=principal_id,
            session_version=session.version,
            checkpoint=checkpoint,
        )

        if target_logical_time_ns == session.logical_time_ns:
            return ReplayAdvanceResult(session, checkpoint, 0, recovered.applied_inputs)

        run = self._run_until(
            setup_manifest_hash=session.setup.manifest_hash,
            warmup_start_ns=session.setup.play_start_ns - session.setup.warmup_ns,
            target_ns=target_logical_time_ns,
            checkpoint=checkpoint,
            batch_size=batch_size,
        )
        advanced = self.lifecycle.advance(
            session_id=session_id,
            principal_id=principal_id,
            expected_version=session.version,
            logical_time_ns=target_logical_time_ns,
        )
        self.checkpoints.save(
            session_id=session_id,
            principal_id=principal_id,
            session_version=advanced.version,
            checkpoint=run.checkpoint,
        )
        return ReplayAdvanceResult(
            advanced,
            run.checkpoint,
            run.applied_inputs,
            recovered.applied_inputs,
        )

    def _recover_current_frontier(
        self,
        *,
        session: SessionRecord,
        checkpoint: ReplayCheckpoint | None,
        batch_size: int,
    ) -> _RunResult:
        assert session.setup is not None
        assert session.logical_time_ns is not None
        if checkpoint is not None and checkpoint.logical_time_ns > session.logical_time_ns:
            raise ReplayError(
                ReplayErrorCode.SNAPSHOT_CORRUPT,
                "checkpoint is ahead of persisted session frontier",
            )
        if checkpoint is not None and checkpoint.logical_time_ns == session.logical_time_ns:
            restored = self.simulator.restore(checkpoint)
            _require_same_state(restored, checkpoint.simulator)
            return _RunResult(checkpoint, 0)
        return self._run_until(
            setup_manifest_hash=session.setup.manifest_hash,
            warmup_start_ns=session.setup.play_start_ns - session.setup.warmup_ns,
            target_ns=session.logical_time_ns,
            checkpoint=checkpoint,
            batch_size=batch_size,
        )

    def _run_until(
        self,
        *,
        setup_manifest_hash: str,
        warmup_start_ns: int,
        target_ns: int,
        checkpoint: ReplayCheckpoint | None,
        batch_size: int,
    ) -> _RunResult:
        state = self.simulator.restore(checkpoint)
        if checkpoint is not None:
            _require_same_state(state, checkpoint.simulator)
        _validate_simulator_state(state)
        after_seq = None if checkpoint is None else checkpoint.source_event_seq
        prior_time = warmup_start_ns if checkpoint is None else checkpoint.logical_time_ns
        applied = 0

        while True:
            batch = self.source.next_after(
                manifest_hash=setup_manifest_hash,
                after_source_event_seq=after_seq,
                through_ns=target_ns,
                limit=batch_size,
            )
            if len(batch) > batch_size:
                raise ReplayError(ReplayErrorCode.SOURCE_SEQUENCE, "source exceeded requested limit")
            if not batch:
                break
            for item in batch:
                self._validate_source_item(item, after_seq, prior_time, warmup_start_ns, target_ns)
                next_state = self.simulator.apply(
                    item,
                    expected_state_version=state.state_version,
                )
                _validate_transition(state, next_state)
                state = next_state
                after_seq = item.source_event_seq
                prior_time = item.logical_ts_ns
                applied += 1
            if len(batch) < batch_size:
                break

        return _RunResult(ReplayCheckpoint(target_ns, after_seq, state), applied)

    @staticmethod
    def _validate_source_item(
        item: ReplayInput,
        after_seq: int | None,
        prior_time: int,
        warmup_start_ns: int,
        target_ns: int,
    ) -> None:
        expected_seq = 0 if after_seq is None else after_seq + 1
        if item.source_event_seq != expected_seq:
            raise ReplayError(
                ReplayErrorCode.SOURCE_SEQUENCE,
                f"expected source sequence {expected_seq}, received {item.source_event_seq}",
            )
        if item.logical_ts_ns < warmup_start_ns or item.logical_ts_ns < prior_time:
            raise ReplayError(ReplayErrorCode.SOURCE_TIME, "source logical time regressed")
        if item.logical_ts_ns > target_ns:
            raise ReplayError(ReplayErrorCode.SOURCE_TIME, "source leaked an input beyond target")


def _require_running(session: SessionRecord) -> None:
    if session.setup is None or session.logical_time_ns is None:
        raise ReplayError(ReplayErrorCode.SESSION_NOT_COMMITTED, "session has no committed setup")
    if session.status is not SessionStatus.RUNNING:
        raise ReplayError(ReplayErrorCode.SESSION_NOT_RUNNING, "session must be running")


def _validate_simulator_state(state: SimulatorState) -> None:
    if state.state_version < 0:
        raise ReplayError(ReplayErrorCode.SIMULATOR_VERSION, "simulator version is negative")
    if len(state.state_hash) != 64 or any(c not in "0123456789abcdef" for c in state.state_hash):
        raise ReplayError(ReplayErrorCode.SIMULATOR_HASH, "simulator hash is malformed")


def _require_same_state(actual: SimulatorState, expected: SimulatorState) -> None:
    if actual != expected:
        raise ReplayError(
            ReplayErrorCode.SNAPSHOT_CORRUPT,
            "simulator restore does not reproduce checkpoint state",
        )


def _validate_transition(previous: SimulatorState, current: SimulatorState) -> None:
    _validate_simulator_state(current)
    if current.state_version != previous.state_version + 1:
        raise ReplayError(
            ReplayErrorCode.SIMULATOR_VERSION,
            "simulator did not advance state version by exactly one",
        )
    if current.state_hash == previous.state_hash:
        raise ReplayError(
            ReplayErrorCode.SIMULATOR_HASH,
            "simulator state hash did not change after accepted input",
        )
