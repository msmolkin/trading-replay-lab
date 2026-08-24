"""Transactional persistence for accepted trading commands."""

from __future__ import annotations

import hashlib
from collections.abc import Mapping
from typing import Any, cast

from sqlalchemy import Engine, insert, select, update
from sqlalchemy.exc import IntegrityError

from trading_replay_api.db.schema import commands, sessions
from trading_replay_api.sessions import SessionStatus

from .model import (
    AcceptedCommand,
    CommandErrorCode,
    CommandServiceError,
    PreparedCommand,
    canonical_json,
)

I64_MIN = -(2**63)
I64_MAX = 2**63 - 1
U64_MAX = 2**64 - 1


class CommandStore:
    """Persist canonical commands with principal, idempotency, and version isolation."""

    def __init__(self, engine: Engine) -> None:
        self.engine = engine

    def accept(
        self,
        *,
        session_id: str,
        principal_id: str,
        idempotency_key: str,
        expected_session_version: int,
        accepted_at_ns: int,
        prepared: PreparedCommand,
    ) -> AcceptedCommand:
        """Accept one command exactly once or return an exact idempotent retry."""
        _validate_identity(session_id, "session_id")
        _validate_identity(principal_id, "principal_id")
        _validate_idempotency_key(idempotency_key)
        _validate_version(expected_session_version)
        _validate_i64(accepted_at_ns, "accepted_at_ns")
        try:
            with self.engine.begin() as connection:
                session = connection.execute(
                    select(
                        sessions.c.principal_id,
                        sessions.c.status,
                        sessions.c.version,
                    )
                    .where(sessions.c.session_id == session_id)
                    .with_for_update()
                ).one_or_none()
                if session is None:
                    raise CommandServiceError(
                        CommandErrorCode.SESSION_NOT_FOUND,
                        "session does not exist",
                    )
                if str(session.principal_id) != principal_id:
                    raise CommandServiceError(
                        CommandErrorCode.PRINCIPAL_MISMATCH,
                        "session belongs to another principal",
                    )

                existing = connection.execute(
                    select(commands).where(
                        commands.c.session_id == session_id,
                        commands.c.idempotency_key == idempotency_key,
                    )
                ).one_or_none()
                if existing is not None:
                    if str(existing.payload_hash) != prepared.payload_hash:
                        raise CommandServiceError(
                            CommandErrorCode.IDEMPOTENCY_CONFLICT,
                            "idempotency key was already used for a different payload",
                        )
                    return _materialize(existing, replayed=True)

                if str(session.status) != SessionStatus.RUNNING.value:
                    raise CommandServiceError(
                        CommandErrorCode.SESSION_NOT_RUNNING,
                        "trading commands require a running replay session",
                    )
                stored_version = int(session.version)
                if stored_version != expected_session_version:
                    raise CommandServiceError(
                        CommandErrorCode.VERSION_CONFLICT,
                        f"expected session version {expected_session_version}, stored {stored_version}",
                    )
                next_version = expected_session_version + 1
                if next_version > U64_MAX:
                    raise CommandServiceError(
                        CommandErrorCode.VERSION_CONFLICT,
                        "session version is exhausted",
                    )

                command_id = _command_id(session_id, idempotency_key, prepared.payload_hash)
                advanced = connection.execute(
                    update(sessions)
                    .where(
                        sessions.c.session_id == session_id,
                        sessions.c.version == expected_session_version,
                        sessions.c.status == SessionStatus.RUNNING.value,
                    )
                    .values(version=next_version)
                )
                if advanced.rowcount != 1:
                    raise CommandServiceError(
                        CommandErrorCode.VERSION_CONFLICT,
                        "session changed while command was accepted",
                    )
                connection.execute(
                    insert(commands).values(
                        command_id=command_id,
                        session_id=session_id,
                        idempotency_key=idempotency_key,
                        payload_hash=prepared.payload_hash,
                        expected_session_version=expected_session_version,
                        accepted_at_ns=accepted_at_ns,
                        payload_json=prepared.payload,
                    )
                )
                return AcceptedCommand(
                    command_id=command_id,
                    session_id=session_id,
                    idempotency_key=idempotency_key,
                    payload_hash=prepared.payload_hash,
                    expected_session_version=expected_session_version,
                    resulting_session_version=next_version,
                    accepted_at_ns=accepted_at_ns,
                    payload=dict(prepared.payload),
                    replayed=False,
                )
        except IntegrityError as error:
            raise CommandServiceError(
                CommandErrorCode.DATABASE_CONFLICT,
                "database constraint rejected command acceptance",
            ) from error

    def get(
        self,
        *,
        session_id: str,
        principal_id: str,
        command_id: str,
    ) -> AcceptedCommand:
        """Read one accepted command without crossing the principal boundary."""
        _validate_identity(session_id, "session_id")
        _validate_identity(principal_id, "principal_id")
        _validate_identity(command_id, "command_id")
        with self.engine.connect() as connection:
            owner = connection.execute(
                select(sessions.c.principal_id).where(sessions.c.session_id == session_id)
            ).scalar_one_or_none()
            if owner is None:
                raise CommandServiceError(
                    CommandErrorCode.SESSION_NOT_FOUND,
                    "session does not exist",
                )
            if str(owner) != principal_id:
                raise CommandServiceError(
                    CommandErrorCode.PRINCIPAL_MISMATCH,
                    "session belongs to another principal",
                )
            row = connection.execute(
                select(commands).where(
                    commands.c.session_id == session_id,
                    commands.c.command_id == command_id,
                )
            ).one_or_none()
            if row is None:
                raise CommandServiceError(
                    CommandErrorCode.COMMAND_NOT_FOUND,
                    "command does not exist",
                )
            return _materialize(row, replayed=False)


def _materialize(row: Any, *, replayed: bool) -> AcceptedCommand:
    expected = int(row.expected_session_version)
    payload = _mapping(row.payload_json)
    return AcceptedCommand(
        command_id=str(row.command_id),
        session_id=str(row.session_id),
        idempotency_key=str(row.idempotency_key),
        payload_hash=str(row.payload_hash),
        expected_session_version=expected,
        resulting_session_version=expected + 1,
        accepted_at_ns=int(row.accepted_at_ns),
        payload=dict(payload),
        replayed=replayed,
    )


def _command_id(session_id: str, idempotency_key: str, payload_hash: str) -> str:
    identity = canonical_json(
        {
            "idempotency_key": idempotency_key,
            "payload_hash": payload_hash,
            "session_id": session_id,
        }
    )
    return f"cmd_{hashlib.sha256(('TRL-COMMAND-ID-v1\\0' + identity).encode()).hexdigest()}"


def _mapping(value: object) -> Mapping[str, object]:
    if not isinstance(value, Mapping) or any(not isinstance(key, str) for key in value):
        raise CommandServiceError(
            CommandErrorCode.DATABASE_CONFLICT,
            "stored command payload is invalid",
        )
    return cast(Mapping[str, object], value)


def _validate_identity(value: str, name: str) -> None:
    if not value or any(character in value for character in "\x00\r\n"):
        raise CommandServiceError(CommandErrorCode.INVALID_COMMAND, f"invalid {name}")


def _validate_idempotency_key(value: str) -> None:
    if not value or len(value) > 200 or any(character in value for character in "\x00\r\n"):
        raise CommandServiceError(
            CommandErrorCode.INVALID_COMMAND,
            "idempotency_key must contain 1..200 safe characters",
        )


def _validate_version(value: int) -> None:
    if isinstance(value, bool) or value < 0 or value > U64_MAX:
        raise CommandServiceError(
            CommandErrorCode.INVALID_COMMAND,
            "expected_session_version must fit unsigned 64-bit integer",
        )


def _validate_i64(value: int, name: str) -> None:
    if isinstance(value, bool) or value < I64_MIN or value > I64_MAX:
        raise CommandServiceError(
            CommandErrorCode.INVALID_COMMAND,
            f"{name} must fit signed 64-bit integer",
        )
