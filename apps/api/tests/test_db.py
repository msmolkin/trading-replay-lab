from __future__ import annotations

from pathlib import Path

import pytest
from sqlalchemy import create_engine

from trading_replay_api.db import (
    ZERO_HASH,
    CommandRecord,
    ConcurrentSessionVersion,
    EventChainConflict,
    EventRecord,
    EventStore,
    IdempotencyConflict,
)


def store(tmp_path: Path) -> EventStore:
    engine = create_engine(f"sqlite+pysqlite:///{tmp_path / 'events.db'}")
    result = EventStore(engine)
    result.create_schema()
    result.create_session(
        session_id="session-1",
        principal_id="principal-1",
        created_at_ns=1,
    )
    return result


def command(*, command_id: str = "cmd-1", key: str = "idem-1", version: int = 0, payload_hash: str = "a" * 64) -> CommandRecord:
    return CommandRecord(
        command_id=command_id,
        session_id="session-1",
        idempotency_key=key,
        payload_hash=payload_hash,
        expected_session_version=version,
        accepted_at_ns=10,
        payload={"command_type": "SET_LEVERAGE", "requested_leverage": 2},
    )


def event(seq: int, *, prior: str, current: str, causation: str = "cmd-1") -> EventRecord:
    return EventRecord(
        event_seq=seq,
        logical_ts_ns=20 + seq,
        event_type="COMMAND_ACCEPTED",
        causation_id=causation,
        correlation_id="corr-1",
        payload={"ok": True},
        prior_event_hash=prior,
        current_event_hash=current,
    )


def test_exact_retry_does_not_duplicate_events(tmp_path: Path) -> None:
    db = store(tmp_path)
    first = db.append_command(command(), (event(0, prior=ZERO_HASH, current="1" * 64),))
    retry = db.append_command(command(), (event(0, prior=ZERO_HASH, current="1" * 64),))
    assert first.replayed is False
    assert retry.replayed is True
    assert retry.command_id == first.command_id
    assert retry.event_seqs == (0,)
    assert db.event_count("session-1") == 1


def test_idempotency_key_with_changed_payload_rejects(tmp_path: Path) -> None:
    db = store(tmp_path)
    db.append_command(command(), (event(0, prior=ZERO_HASH, current="1" * 64),))
    with pytest.raises(IdempotencyConflict):
        db.append_command(command(command_id="cmd-2", payload_hash="b" * 64), ())
    assert db.event_count("session-1") == 1


def test_stale_version_is_atomic(tmp_path: Path) -> None:
    db = store(tmp_path)
    db.append_command(command(), (event(0, prior=ZERO_HASH, current="1" * 64),))
    stale = command(command_id="cmd-2", key="idem-2", version=0, payload_hash="b" * 64)
    with pytest.raises(ConcurrentSessionVersion):
        db.append_command(stale, (event(1, prior="1" * 64, current="2" * 64, causation="cmd-2"),))
    assert db.event_count("session-1") == 1


def test_broken_hash_chain_rolls_back_command_and_version(tmp_path: Path) -> None:
    db = store(tmp_path)
    with pytest.raises(EventChainConflict):
        db.append_command(command(), (event(0, prior="f" * 64, current="1" * 64),))
    assert db.event_count("session-1") == 0
    # Version zero remains usable after the failed transaction.
    accepted = db.append_command(command(), (event(0, prior=ZERO_HASH, current="1" * 64),))
    assert accepted.session_version == 1


def test_migration_pair_exists() -> None:
    root = Path(__file__).resolve().parents[3]
    up = root / "migrations" / "0001_event_store.up.sql"
    down = root / "migrations" / "0001_event_store.down.sql"
    assert "CREATE TABLE domain_events" in up.read_text(encoding="utf-8")
    assert "DROP TABLE IF EXISTS domain_events" in down.read_text(encoding="utf-8")
