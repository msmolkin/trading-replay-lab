"""Persistence adapters for deterministic replay checkpoints and event publication."""

from __future__ import annotations

import json
from collections.abc import Mapping
from typing import Any, cast

from sqlalchemy import Engine, insert, select
from sqlalchemy.exc import IntegrityError

from trading_replay_api.db.schema import domain_events, sessions, snapshots

from .model import (
    PersistedReplayEvent,
    ReplayCheckpoint,
    ReplayError,
    ReplayErrorCode,
    SimulatorState,
    canonical_json,
)

CHECKPOINT_FORMAT = "1"


class ReplayCheckpointStore:
    """Principal-scoped checkpoint storage over the shared snapshot table."""

    def __init__(self, engine: Engine) -> None:
        self.engine = engine

    def load_latest(self, *, session_id: str, principal_id: str) -> ReplayCheckpoint | None:
        """Return the latest verified-format checkpoint for one owned session."""
        with self.engine.connect() as connection:
            _require_principal(connection, session_id, principal_id)
            row = connection.execute(
                select(
                    snapshots.c.event_seq,
                    snapshots.c.state_version,
                    snapshots.c.state_hash,
                    snapshots.c.state_json,
                )
                .where(snapshots.c.session_id == session_id)
                .order_by(snapshots.c.event_seq.desc())
                .limit(1)
            ).one_or_none()
            if row is None:
                return None
            return _decode_checkpoint(row.state_json, int(row.state_version), str(row.state_hash))

    def save(
        self,
        *,
        session_id: str,
        principal_id: str,
        session_version: int,
        checkpoint: ReplayCheckpoint,
    ) -> None:
        """Persist an immutable checkpoint or accept an exact idempotent retry."""
        if isinstance(session_version, bool) or session_version <= 0:
            raise ValueError("session_version must be a positive integer")
        state_json = _checkpoint_payload(checkpoint)
        try:
            with self.engine.begin() as connection:
                _require_principal(connection, session_id, principal_id)
                existing = connection.execute(
                    select(
                        snapshots.c.state_version,
                        snapshots.c.state_hash,
                        snapshots.c.state_json,
                    ).where(
                        snapshots.c.session_id == session_id,
                        snapshots.c.event_seq == session_version,
                    )
                ).one_or_none()
                if existing is not None:
                    if (
                        int(existing.state_version) == checkpoint.simulator.state_version
                        and str(existing.state_hash) == checkpoint.simulator.state_hash
                        and canonical_json(_mapping(existing.state_json)) == canonical_json(state_json)
                    ):
                        return
                    raise ReplayError(
                        ReplayErrorCode.SNAPSHOT_CONFLICT,
                        "session version already has a different replay checkpoint",
                    )
                connection.execute(
                    insert(snapshots).values(
                        session_id=session_id,
                        event_seq=session_version,
                        state_version=checkpoint.simulator.state_version,
                        state_hash=checkpoint.simulator.state_hash,
                        state_json=state_json,
                    )
                )
        except IntegrityError as error:
            raise ReplayError(
                ReplayErrorCode.SNAPSHOT_CONFLICT,
                "concurrent checkpoint write conflicted",
            ) from error


class PersistedEventPublisher:
    """Read only persisted events for websocket/fan-out consumers."""

    def __init__(self, engine: Engine) -> None:
        self.engine = engine

    def read_after(
        self,
        *,
        session_id: str,
        principal_id: str,
        after_event_seq: int | None,
        limit: int = 256,
    ) -> tuple[PersistedReplayEvent, ...]:
        """Read a monotonic principal-scoped page from the persisted event stream."""
        if after_event_seq is not None and (isinstance(after_event_seq, bool) or after_event_seq < 0):
            raise ValueError("after_event_seq must be nonnegative")
        if isinstance(limit, bool) or limit <= 0 or limit > 10_000:
            raise ValueError("limit must be between 1 and 10000")
        with self.engine.connect() as connection:
            _require_principal(connection, session_id, principal_id)
            statement = select(
                domain_events.c.event_seq,
                domain_events.c.logical_ts_ns,
                domain_events.c.event_type,
                domain_events.c.payload_json,
                domain_events.c.current_event_hash,
            ).where(domain_events.c.session_id == session_id)
            if after_event_seq is not None:
                statement = statement.where(domain_events.c.event_seq > after_event_seq)
            rows = connection.execute(statement.order_by(domain_events.c.event_seq).limit(limit))
            return tuple(
                PersistedReplayEvent(
                    event_seq=int(row.event_seq),
                    logical_ts_ns=int(row.logical_ts_ns),
                    event_type=str(row.event_type),
                    payload=dict(_mapping(row.payload_json)),
                    current_event_hash=str(row.current_event_hash),
                )
                for row in rows
            )


def _checkpoint_payload(checkpoint: ReplayCheckpoint) -> dict[str, object]:
    return {
        "checkpoint_format": CHECKPOINT_FORMAT,
        "logical_time_ns": str(checkpoint.logical_time_ns),
        "simulator_snapshot": checkpoint.simulator.snapshot(),
        "source_event_seq": (
            None if checkpoint.source_event_seq is None else str(checkpoint.source_event_seq)
        ),
    }


def _decode_checkpoint(raw: object, state_version: int, state_hash: str) -> ReplayCheckpoint:
    try:
        payload = _mapping(raw)
        if payload.get("checkpoint_format") != CHECKPOINT_FORMAT:
            raise ValueError("unsupported checkpoint format")
        logical_time_ns = _canonical_int(payload.get("logical_time_ns"), "logical_time_ns")
        source_raw = payload.get("source_event_seq")
        source_event_seq = (
            None if source_raw is None else _canonical_int(source_raw, "source_event_seq")
        )
        snapshot_payload = _mapping(payload.get("simulator_snapshot"))
        simulator = SimulatorState.from_snapshot(
            state_version=state_version,
            state_hash=state_hash,
            snapshot=snapshot_payload,
        )
        return ReplayCheckpoint(logical_time_ns, source_event_seq, simulator)
    except (TypeError, ValueError, ReplayError) as error:
        if isinstance(error, ReplayError):
            raise
        raise ReplayError(ReplayErrorCode.SNAPSHOT_CORRUPT, "stored replay checkpoint is invalid") from error


def _canonical_int(value: object, name: str) -> int:
    if not isinstance(value, str) or not value or value.startswith("+"):
        raise ValueError(f"{name} must be canonical decimal text")
    if value == "-0" or (value.startswith("0") and value != "0"):
        raise ValueError(f"{name} must be canonical decimal text")
    digits = value[1:] if value.startswith("-") else value
    if not digits.isdigit() or (value.startswith("-") and digits.startswith("0")):
        raise ValueError(f"{name} must be canonical decimal text")
    return int(value)


def _mapping(value: object) -> Mapping[str, object]:
    if not isinstance(value, Mapping):
        raise ValueError("expected JSON object")
    if any(not isinstance(key, str) for key in value):
        raise ValueError("JSON object keys must be strings")
    return cast(Mapping[str, object], value)


def _require_principal(connection: Any, session_id: str, principal_id: str) -> None:
    owner = connection.execute(
        select(sessions.c.principal_id).where(sessions.c.session_id == session_id)
    ).scalar_one_or_none()
    if owner is None or str(owner) != principal_id:
        raise ReplayError(ReplayErrorCode.SNAPSHOT_CORRUPT, "session is unavailable to principal")
