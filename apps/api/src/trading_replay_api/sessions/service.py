"""Transactional setup validation and replay-session lifecycle."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Mapping
from typing import cast

from sqlalchemy import Connection, Engine, insert, select, update
from sqlalchemy.exc import IntegrityError

from trading_replay_api.catalog import CoverageCatalog, SetupRequirement
from trading_replay_api.db.schema import domain_events, metadata, rulesets, sessions
from trading_replay_api.db.store import ZERO_HASH

from .model import (
    CommittedSetup,
    SessionErrorCode,
    SessionLifecycleError,
    SessionRecord,
    SessionStatus,
    SetupRequest,
    capabilities_for_tier,
    validate_i64,
    validate_version,
)


class SessionService:
    """Own setup commitment and lifecycle transitions over the shared event-store schema."""

    def __init__(self, engine: Engine, catalog: CoverageCatalog) -> None:
        self.engine = engine
        self.catalog = catalog

    def create_schema(self) -> None:
        """Create the reference schema for tests/local use; deployments use migrations."""
        metadata.create_all(self.engine)

    def create_session(
        self,
        *,
        session_id: str,
        principal_id: str,
        created_at_ns: int,
    ) -> SessionRecord:
        """Create an uncommitted setup session at optimistic version zero."""
        _validate_identity(session_id, "session_id")
        _validate_identity(principal_id, "principal_id")
        validate_i64(created_at_ns, "created_at_ns")
        try:
            with self.engine.begin() as connection:
                connection.execute(
                    insert(sessions).values(
                        session_id=session_id,
                        principal_id=principal_id,
                        status=SessionStatus.SETUP.value,
                        version=0,
                        created_at_ns=created_at_ns,
                    )
                )
                return self._materialize(connection, session_id, principal_id)
        except IntegrityError as error:
            raise SessionLifecycleError(
                SessionErrorCode.SESSION_EXISTS,
                f"session {session_id!r} already exists",
            ) from error

    def get_session(self, *, session_id: str, principal_id: str) -> SessionRecord:
        """Read materialized state while enforcing principal ownership."""
        with self.engine.connect() as connection:
            return self._materialize(connection, session_id, principal_id)

    def commit(
        self,
        *,
        session_id: str,
        principal_id: str,
        expected_version: int,
        setup: SetupRequest,
    ) -> SessionRecord:
        """Validate and immutably pin one eligible manifest/ruleset setup."""
        next_version = _next_version(expected_version)
        if setup.execution_tier not in setup.ruleset.allowed_execution_tiers:
            raise SessionLifecycleError(
                SessionErrorCode.RULESET_FIDELITY_UNSUPPORTED,
                f"ruleset {setup.ruleset.ruleset_id!r} does not allow {setup.execution_tier.value}",
            )

        required_capabilities = capabilities_for_tier(setup.execution_tier).union(
            setup.required_capabilities
        )
        requirement = SetupRequirement(
            instrument_id=setup.instrument_id,
            play_start_ns=setup.play_start_ns,
            warmup_ns=setup.warmup_ns,
            duration_ns=setup.duration_ns,
            minimum_tier=setup.execution_tier,
            required_capabilities=required_capabilities,
            allowed_redistribution=setup.allowed_redistribution,
            allow_degraded=setup.allow_degraded,
        )
        eligible = self.catalog.eligible_setups(requirement)
        if not eligible:
            raise SessionLifecycleError(
                SessionErrorCode.SETUP_INELIGIBLE,
                "no single gap-free manifest satisfies the requested setup",
            )
        if setup.manifest_hash not in {item.manifest_hash for item in eligible}:
            raise SessionLifecycleError(
                SessionErrorCode.MANIFEST_INELIGIBLE,
                "requested manifest is not in the deterministic eligible set",
            )

        committed = CommittedSetup(
            instrument_id=setup.instrument_id,
            manifest_hash=setup.manifest_hash,
            eligibility_hash=self.catalog.eligibility_hash(requirement),
            play_start_ns=setup.play_start_ns,
            warmup_ns=setup.warmup_ns,
            duration_ns=setup.duration_ns,
            execution_tier=setup.execution_tier,
            required_capabilities=frozenset(required_capabilities),
            allowed_redistribution=setup.allowed_redistribution,
            allow_degraded=setup.allow_degraded,
            visibility_mode=setup.visibility_mode,
            ruleset_id=setup.ruleset.ruleset_id,
            ruleset_version=setup.ruleset.version,
            ruleset_hash=setup.ruleset.ruleset_hash,
        )

        with self.engine.begin() as connection:
            row = self._locked_row(connection, session_id, principal_id)
            _require_version(row.version, expected_version)
            _require_status(SessionStatus(str(row.status)), {SessionStatus.SETUP})
            self._persist_ruleset(connection, setup)
            self._append_event(
                connection,
                session_id=session_id,
                event_type="SESSION_COMMITTED",
                logical_ts_ns=setup.play_start_ns,
                payload=committed.to_payload(),
                state_version=next_version,
            )
            advanced = connection.execute(
                update(sessions)
                .where(
                    sessions.c.session_id == session_id,
                    sessions.c.version == expected_version,
                    sessions.c.status == SessionStatus.SETUP.value,
                )
                .values(
                    status=SessionStatus.COMMITTED.value,
                    version=next_version,
                    ruleset_id=setup.ruleset.ruleset_id,
                )
            )
            if advanced.rowcount != 1:
                raise SessionLifecycleError(
                    SessionErrorCode.VERSION_CONFLICT,
                    "session changed while setup was being committed",
                )
            return self._materialize(connection, session_id, principal_id)

    def start(
        self,
        *,
        session_id: str,
        principal_id: str,
        expected_version: int,
    ) -> SessionRecord:
        """Start a committed session or resume a paused one."""
        return self._status_transition(
            session_id=session_id,
            principal_id=principal_id,
            expected_version=expected_version,
            allowed={SessionStatus.COMMITTED, SessionStatus.PAUSED},
            target=SessionStatus.RUNNING,
            event_type="SESSION_STARTED",
        )

    def pause(
        self,
        *,
        session_id: str,
        principal_id: str,
        expected_version: int,
    ) -> SessionRecord:
        """Pause a running replay at its current logical frontier."""
        return self._status_transition(
            session_id=session_id,
            principal_id=principal_id,
            expected_version=expected_version,
            allowed={SessionStatus.RUNNING},
            target=SessionStatus.PAUSED,
            event_type="SESSION_PAUSED",
        )

    def advance(
        self,
        *,
        session_id: str,
        principal_id: str,
        expected_version: int,
        logical_time_ns: int,
    ) -> SessionRecord:
        """Advance a running session monotonically without crossing its committed end."""
        validate_i64(logical_time_ns, "logical_time_ns")
        next_version = _next_version(expected_version)
        with self.engine.begin() as connection:
            row = self._locked_row(connection, session_id, principal_id)
            _require_version(row.version, expected_version)
            current = self._materialize_from_row(connection, row, principal_id)
            _require_status(current.status, {SessionStatus.RUNNING})
            if current.setup is None or current.logical_time_ns is None:
                raise SessionLifecycleError(
                    SessionErrorCode.INVALID_TRANSITION,
                    "running session has no committed setup",
                )
            if (
                logical_time_ns <= current.logical_time_ns
                or logical_time_ns > current.setup.play_end_ns
            ):
                raise SessionLifecycleError(
                    SessionErrorCode.ADVANCE_OUT_OF_RANGE,
                    "logical time must advance monotonically and not exceed the committed end",
                )
            self._append_event(
                connection,
                session_id=session_id,
                event_type="SESSION_ADVANCED",
                logical_ts_ns=logical_time_ns,
                payload={"logical_time_ns": str(logical_time_ns)},
                state_version=next_version,
            )
            self._advance_version(
                connection,
                session_id=session_id,
                expected_version=expected_version,
                next_version=next_version,
                target_status=SessionStatus.RUNNING,
            )
            return self._materialize(connection, session_id, principal_id)

    def complete(
        self,
        *,
        session_id: str,
        principal_id: str,
        expected_version: int,
    ) -> SessionRecord:
        """Complete only after the logical frontier reaches the committed end exactly."""
        next_version = _next_version(expected_version)
        with self.engine.begin() as connection:
            row = self._locked_row(connection, session_id, principal_id)
            _require_version(row.version, expected_version)
            current = self._materialize_from_row(connection, row, principal_id)
            _require_status(current.status, {SessionStatus.RUNNING, SessionStatus.PAUSED})
            if (
                current.setup is None
                or current.logical_time_ns is None
                or current.logical_time_ns != current.setup.play_end_ns
            ):
                raise SessionLifecycleError(
                    SessionErrorCode.INVALID_TRANSITION,
                    "session cannot complete before the committed replay end",
                )
            self._append_event(
                connection,
                session_id=session_id,
                event_type="SESSION_COMPLETED",
                logical_ts_ns=current.logical_time_ns,
                payload={},
                state_version=next_version,
            )
            self._advance_version(
                connection,
                session_id=session_id,
                expected_version=expected_version,
                next_version=next_version,
                target_status=SessionStatus.COMPLETED,
            )
            return self._materialize(connection, session_id, principal_id)

    def fork(
        self,
        *,
        session_id: str,
        principal_id: str,
        expected_version: int,
        child_session_id: str,
        created_at_ns: int,
    ) -> SessionRecord:
        """Fork a stable committed frontier into a new paused/committed child session."""
        _validate_identity(child_session_id, "child_session_id")
        validate_i64(created_at_ns, "created_at_ns")
        validate_version(expected_version)
        if child_session_id == session_id:
            raise SessionLifecycleError(
                SessionErrorCode.INVALID_FORK_TARGET,
                "fork target must use a different session id",
            )

        try:
            with self.engine.begin() as connection:
                row = self._locked_row(connection, session_id, principal_id)
                _require_version(row.version, expected_version)
                current = self._materialize_from_row(connection, row, principal_id)
                _require_status(
                    current.status,
                    {SessionStatus.COMMITTED, SessionStatus.RUNNING, SessionStatus.PAUSED},
                )
                if current.setup is None or current.logical_time_ns is None:
                    raise SessionLifecycleError(
                        SessionErrorCode.INVALID_TRANSITION,
                        "only committed sessions can be forked",
                    )
                child_status = (
                    SessionStatus.COMMITTED
                    if current.logical_time_ns == current.setup.play_start_ns
                    else SessionStatus.PAUSED
                )
                connection.execute(
                    insert(sessions).values(
                        session_id=child_session_id,
                        principal_id=principal_id,
                        status=child_status.value,
                        version=1,
                        ruleset_id=current.setup.ruleset_id,
                        created_at_ns=created_at_ns,
                    )
                )
                payload = current.setup.to_payload()
                payload.update(
                    {
                        "fork_logical_time_ns": str(current.logical_time_ns),
                        "parent_session_id": session_id,
                        "parent_version": str(expected_version),
                    }
                )
                self._append_event(
                    connection,
                    session_id=child_session_id,
                    event_type="SESSION_FORKED",
                    logical_ts_ns=current.logical_time_ns,
                    payload=payload,
                    state_version=1,
                )
                return self._materialize(connection, child_session_id, principal_id)
        except IntegrityError as error:
            raise SessionLifecycleError(
                SessionErrorCode.SESSION_EXISTS,
                f"fork target {child_session_id!r} already exists",
            ) from error

    def _status_transition(
        self,
        *,
        session_id: str,
        principal_id: str,
        expected_version: int,
        allowed: set[SessionStatus],
        target: SessionStatus,
        event_type: str,
    ) -> SessionRecord:
        next_version = _next_version(expected_version)
        with self.engine.begin() as connection:
            row = self._locked_row(connection, session_id, principal_id)
            _require_version(row.version, expected_version)
            current = self._materialize_from_row(connection, row, principal_id)
            _require_status(current.status, allowed)
            if current.logical_time_ns is None:
                raise SessionLifecycleError(
                    SessionErrorCode.INVALID_TRANSITION,
                    "session has no committed logical frontier",
                )
            self._append_event(
                connection,
                session_id=session_id,
                event_type=event_type,
                logical_ts_ns=current.logical_time_ns,
                payload={},
                state_version=next_version,
            )
            self._advance_version(
                connection,
                session_id=session_id,
                expected_version=expected_version,
                next_version=next_version,
                target_status=target,
            )
            return self._materialize(connection, session_id, principal_id)

    def _advance_version(
        self,
        connection: Connection,
        *,
        session_id: str,
        expected_version: int,
        next_version: int,
        target_status: SessionStatus,
    ) -> None:
        advanced = connection.execute(
            update(sessions)
            .where(
                sessions.c.session_id == session_id,
                sessions.c.version == expected_version,
            )
            .values(status=target_status.value, version=next_version)
        )
        if advanced.rowcount != 1:
            raise SessionLifecycleError(
                SessionErrorCode.VERSION_CONFLICT,
                "session changed during lifecycle transition",
            )

    def _locked_row(self, connection: Connection, session_id: str, principal_id: str) -> object:
        row = connection.execute(
            select(sessions).where(sessions.c.session_id == session_id).with_for_update()
        ).one_or_none()
        if row is None:
            raise SessionLifecycleError(
                SessionErrorCode.SESSION_NOT_FOUND,
                f"session {session_id!r} does not exist",
            )
        if str(row.principal_id) != principal_id:
            raise SessionLifecycleError(
                SessionErrorCode.PRINCIPAL_MISMATCH,
                "session is owned by a different principal",
            )
        return row

    def _materialize(
        self,
        connection: Connection,
        session_id: str,
        principal_id: str,
    ) -> SessionRecord:
        row = connection.execute(
            select(sessions).where(sessions.c.session_id == session_id)
        ).one_or_none()
        if row is None:
            raise SessionLifecycleError(
                SessionErrorCode.SESSION_NOT_FOUND,
                f"session {session_id!r} does not exist",
            )
        return self._materialize_from_row(connection, row, principal_id)

    def _materialize_from_row(
        self,
        connection: Connection,
        row: object,
        principal_id: str,
    ) -> SessionRecord:
        if str(row.principal_id) != principal_id:
            raise SessionLifecycleError(
                SessionErrorCode.PRINCIPAL_MISMATCH,
                "session is owned by a different principal",
            )
        session_id = str(row.session_id)
        setup: CommittedSetup | None = None
        logical_time_ns: int | None = None
        parent_session_id: str | None = None
        event_rows = connection.execute(
            select(domain_events.c.event_type, domain_events.c.payload_json)
            .where(domain_events.c.session_id == session_id)
            .order_by(domain_events.c.event_seq)
        )
        try:
            for event in event_rows:
                payload = _mapping(event.payload_json)
                event_type = str(event.event_type)
                if event_type in {"SESSION_COMMITTED", "SESSION_FORKED"}:
                    if setup is not None:
                        raise ValueError("session has multiple setup-pin events")
                    setup = CommittedSetup.from_payload(payload)
                    logical_time_ns = setup.play_start_ns
                    if event_type == "SESSION_FORKED":
                        parent_session_id = _required_string(payload, "parent_session_id")
                        logical_time_ns = _decimal_i64(payload, "fork_logical_time_ns")
                elif event_type == "SESSION_ADVANCED":
                    if setup is None:
                        raise ValueError("advance precedes setup commit")
                    logical_time_ns = _decimal_i64(payload, "logical_time_ns")
        except (TypeError, ValueError) as error:
            raise SessionLifecycleError(
                SessionErrorCode.INVALID_VALUE,
                "persisted session event payload is invalid",
            ) from error

        version = int(row.version)
        validate_version(version)
        created_at_ns = int(row.created_at_ns)
        validate_i64(created_at_ns, "created_at_ns")
        status = SessionStatus(str(row.status))
        if status != SessionStatus.SETUP and setup is None:
            raise SessionLifecycleError(
                SessionErrorCode.INVALID_VALUE,
                "persisted non-setup session is missing its immutable setup event",
            )
        if setup is not None and row.ruleset_id is not None and str(row.ruleset_id) != setup.ruleset_id:
            raise SessionLifecycleError(
                SessionErrorCode.INVALID_VALUE,
                "persisted ruleset id disagrees with the immutable setup event",
            )
        return SessionRecord(
            session_id=session_id,
            principal_id=principal_id,
            status=status,
            version=version,
            created_at_ns=created_at_ns,
            setup=setup,
            logical_time_ns=logical_time_ns,
            parent_session_id=parent_session_id,
        )

    def _persist_ruleset(self, connection: Connection, setup: SetupRequest) -> None:
        definition = setup.ruleset
        existing = connection.execute(
            select(
                rulesets.c.ruleset_version,
                rulesets.c.ruleset_hash,
                rulesets.c.body_json,
            ).where(rulesets.c.ruleset_id == definition.ruleset_id)
        ).one_or_none()
        body = definition.body()
        if existing is None:
            connection.execute(
                insert(rulesets).values(
                    ruleset_id=definition.ruleset_id,
                    ruleset_version=definition.version,
                    ruleset_hash=definition.ruleset_hash,
                    body_json=body,
                )
            )
            return
        if (
            str(existing.ruleset_version) != definition.version
            or str(existing.ruleset_hash) != definition.ruleset_hash
            or _mapping(existing.body_json) != body
        ):
            raise SessionLifecycleError(
                SessionErrorCode.RULESET_CONFLICT,
                "ruleset id is already bound to different immutable content",
            )

    def _append_event(
        self,
        connection: Connection,
        *,
        session_id: str,
        event_type: str,
        logical_ts_ns: int,
        payload: dict[str, object],
        state_version: int,
    ) -> None:
        validate_i64(logical_ts_ns, "logical_ts_ns")
        validate_version(state_version)
        last = connection.execute(
            select(domain_events.c.event_seq, domain_events.c.current_event_hash)
            .where(domain_events.c.session_id == session_id)
            .order_by(domain_events.c.event_seq.desc())
            .limit(1)
        ).one_or_none()
        event_seq = 0 if last is None else int(last.event_seq) + 1
        validate_version(event_seq)
        prior_hash = ZERO_HASH if last is None else str(last.current_event_hash)
        causation_id = f"session-lifecycle:{session_id}:v{state_version}"
        current_hash = _event_hash(
            session_id=session_id,
            event_seq=event_seq,
            logical_ts_ns=logical_ts_ns,
            event_type=event_type,
            causation_id=causation_id,
            payload=payload,
            prior_hash=prior_hash,
        )
        connection.execute(
            insert(domain_events).values(
                session_id=session_id,
                event_seq=event_seq,
                logical_ts_ns=logical_ts_ns,
                event_type=event_type,
                causation_id=causation_id,
                correlation_id=session_id,
                payload_json=payload,
                prior_event_hash=prior_hash,
                current_event_hash=current_hash,
            )
        )


def _event_hash(
    *,
    session_id: str,
    event_seq: int,
    logical_ts_ns: int,
    event_type: str,
    causation_id: str,
    payload: Mapping[str, object],
    prior_hash: str,
) -> str:
    canonical = json.dumps(
        {
            "causation_id": causation_id,
            "event_seq": str(event_seq),
            "event_type": event_type,
            "logical_ts_ns": str(logical_ts_ns),
            "payload": payload,
            "prior_event_hash": prior_hash,
            "session_id": session_id,
        },
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=True,
        allow_nan=False,
    ).encode("ascii")
    return hashlib.sha256(canonical).hexdigest()


def _next_version(expected_version: int) -> int:
    try:
        validate_version(expected_version)
    except ValueError as error:
        raise SessionLifecycleError(SessionErrorCode.INVALID_VALUE, str(error)) from error
    if expected_version == 2**64 - 1:
        raise SessionLifecycleError(
            SessionErrorCode.VERSION_CONFLICT,
            "session version cannot advance beyond uint64",
        )
    return expected_version + 1


def _require_version(stored: object, expected: int) -> None:
    try:
        validate_version(expected)
        stored_version = int(stored)
        validate_version(stored_version)
    except (TypeError, ValueError) as error:
        raise SessionLifecycleError(SessionErrorCode.INVALID_VALUE, "invalid session version") from error
    if stored_version != expected:
        raise SessionLifecycleError(
            SessionErrorCode.VERSION_CONFLICT,
            f"expected session version {expected}, stored {stored_version}",
        )


def _require_status(status: SessionStatus, allowed: set[SessionStatus]) -> None:
    if status not in allowed:
        accepted = ",".join(sorted(item.value for item in allowed))
        raise SessionLifecycleError(
            SessionErrorCode.INVALID_TRANSITION,
            f"status {status.value} cannot perform transition; expected one of {accepted}",
        )


def _validate_identity(value: str, name: str) -> None:
    if not value or len(value) > 160 or any(character.isspace() for character in value):
        raise SessionLifecycleError(
            SessionErrorCode.INVALID_VALUE,
            f"{name} must be 1-160 non-whitespace characters",
        )


def _mapping(value: object) -> dict[str, object]:
    if not isinstance(value, dict) or any(not isinstance(key, str) for key in value):
        raise ValueError("JSON payload must be an object with string keys")
    return cast(dict[str, object], value)


def _required_string(payload: Mapping[str, object], key: str) -> str:
    value = payload.get(key)
    if not isinstance(value, str) or not value:
        raise ValueError(f"{key} must be a non-empty string")
    return value


def _decimal_i64(payload: Mapping[str, object], key: str) -> int:
    raw = _required_string(payload, key)
    if raw == "-0" or raw.startswith("+") or (raw.startswith("0") and raw != "0"):
        raise ValueError(f"{key} must use canonical decimal encoding")
    negative = raw.startswith("-")
    digits = raw[1:] if negative else raw
    if not digits.isdigit() or (negative and digits.startswith("0")):
        raise ValueError(f"{key} must use canonical decimal encoding")
    value = int(raw)
    validate_i64(value, key)
    return value
