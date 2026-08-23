"""Transactional event-store operations with idempotency and optimistic versioning."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from sqlalchemy import Connection, Engine, func, insert, select, update
from sqlalchemy.exc import IntegrityError

from .schema import commands, domain_events, metadata, sessions

ZERO_HASH = "0" * 64


class EventStoreError(RuntimeError):
    """Base class for event-store write failures."""


class SessionNotFound(EventStoreError):
    """Raised when a command targets a missing session."""


class ConcurrentSessionVersion(EventStoreError):
    """Raised when optimistic session versioning rejects a stale command."""


class IdempotencyConflict(EventStoreError):
    """Raised when an idempotency key is reused with a different payload."""


class EventChainConflict(EventStoreError):
    """Raised when appended events are not contiguous with the stored hash chain."""


@dataclass(frozen=True, slots=True)
class EventRecord:
    """Canonical append-only event fields persisted atomically with a command."""

    event_seq: int
    logical_ts_ns: int
    event_type: str
    causation_id: str
    correlation_id: str
    payload: dict[str, Any]
    prior_event_hash: str
    current_event_hash: str


@dataclass(frozen=True, slots=True)
class CommandRecord:
    """Canonical command fields used for idempotent persistence."""

    command_id: str
    session_id: str
    idempotency_key: str
    payload_hash: str
    expected_session_version: int
    accepted_at_ns: int
    payload: dict[str, Any]


@dataclass(frozen=True, slots=True)
class AppendResult:
    """Result of an append or an exact idempotent retry."""

    command_id: str
    session_version: int
    event_seqs: tuple[int, ...]
    replayed: bool


class EventStore:
    """Append-only session event store over a SQLAlchemy engine."""

    def __init__(self, engine: Engine) -> None:
        self.engine = engine

    def create_schema(self) -> None:
        """Create the reference schema; production deployments use migrations."""
        metadata.create_all(self.engine)

    def create_session(
        self,
        *,
        session_id: str,
        principal_id: str,
        created_at_ns: int,
        status: str = "SETUP",
    ) -> None:
        """Create an empty session at optimistic version zero."""
        with self.engine.begin() as connection:
            connection.execute(
                insert(sessions).values(
                    session_id=session_id,
                    principal_id=principal_id,
                    status=status,
                    version=0,
                    created_at_ns=created_at_ns,
                )
            )

    def _idempotent_retry(
        self,
        connection: Connection,
        command: CommandRecord,
    ) -> AppendResult | None:
        row = connection.execute(
            select(commands.c.command_id, commands.c.payload_hash).where(
                commands.c.session_id == command.session_id,
                commands.c.idempotency_key == command.idempotency_key,
            )
        ).one_or_none()
        if row is None:
            return None
        if row.payload_hash != command.payload_hash:
            raise IdempotencyConflict(command.idempotency_key)
        version = connection.execute(
            select(sessions.c.version).where(sessions.c.session_id == command.session_id)
        ).scalar_one()
        event_seqs = tuple(
            int(value)
            for value in connection.execute(
                select(domain_events.c.event_seq)
                .where(
                    domain_events.c.session_id == command.session_id,
                    domain_events.c.causation_id == row.command_id,
                )
                .order_by(domain_events.c.event_seq)
            ).scalars()
        )
        return AppendResult(row.command_id, int(version), event_seqs, True)

    def append_command(
        self,
        command: CommandRecord,
        events: tuple[EventRecord, ...],
    ) -> AppendResult:
        """Persist one command and its events in one transaction.

        Exact idempotent retries return the original command/event result. Reuse of the
        key with another payload, a stale session version, a sequence gap, or a broken
        prior hash fails without committing any partial write.
        """
        try:
            with self.engine.begin() as connection:
                replay = self._idempotent_retry(connection, command)
                if replay is not None:
                    return replay

                current_version = connection.execute(
                    select(sessions.c.version).where(sessions.c.session_id == command.session_id)
                ).scalar_one_or_none()
                if current_version is None:
                    raise SessionNotFound(command.session_id)

                last = connection.execute(
                    select(domain_events.c.event_seq, domain_events.c.current_event_hash)
                    .where(domain_events.c.session_id == command.session_id)
                    .order_by(domain_events.c.event_seq.desc())
                    .limit(1)
                ).one_or_none()
                expected_seq = 0 if last is None else int(last.event_seq) + 1
                expected_prior_hash = ZERO_HASH if last is None else str(last.current_event_hash)
                for event in events:
                    if event.event_seq != expected_seq or event.prior_event_hash != expected_prior_hash:
                        raise EventChainConflict(
                            f"expected seq/hash {expected_seq}/{expected_prior_hash}, "
                            f"received {event.event_seq}/{event.prior_event_hash}"
                        )
                    expected_seq += 1
                    expected_prior_hash = event.current_event_hash

                advanced = connection.execute(
                    update(sessions)
                    .where(
                        sessions.c.session_id == command.session_id,
                        sessions.c.version == command.expected_session_version,
                    )
                    .values(version=command.expected_session_version + 1)
                )
                if advanced.rowcount != 1:
                    raise ConcurrentSessionVersion(
                        f"expected {command.expected_session_version}, stored {current_version}"
                    )

                connection.execute(
                    insert(commands).values(
                        command_id=command.command_id,
                        session_id=command.session_id,
                        idempotency_key=command.idempotency_key,
                        payload_hash=command.payload_hash,
                        expected_session_version=command.expected_session_version,
                        accepted_at_ns=command.accepted_at_ns,
                        payload_json=command.payload,
                    )
                )
                if events:
                    connection.execute(
                        insert(domain_events),
                        [
                            {
                                "session_id": command.session_id,
                                "event_seq": event.event_seq,
                                "logical_ts_ns": event.logical_ts_ns,
                                "event_type": event.event_type,
                                "causation_id": event.causation_id,
                                "correlation_id": event.correlation_id,
                                "payload_json": event.payload,
                                "prior_event_hash": event.prior_event_hash,
                                "current_event_hash": event.current_event_hash,
                            }
                            for event in events
                        ],
                    )
                return AppendResult(
                    command.command_id,
                    command.expected_session_version + 1,
                    tuple(event.event_seq for event in events),
                    False,
                )
        except IntegrityError as error:
            # Unique constraints are a second line of defense against concurrent duplicate
            # command/event insertion. The transaction is rolled back by Engine.begin().
            raise EventStoreError("database integrity constraint rejected append") from error

    def event_count(self, session_id: str) -> int:
        """Return persisted event count for tests/read-model handoff."""
        with self.engine.connect() as connection:
            count = connection.execute(
                select(func.count()).select_from(domain_events).where(
                    domain_events.c.session_id == session_id
                )
            ).scalar_one()
            return int(count)
