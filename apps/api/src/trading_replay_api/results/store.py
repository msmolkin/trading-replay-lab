"""Principal-scoped authoritative reads and write-once result persistence."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any, cast

from sqlalchemy import Engine, insert, select
from sqlalchemy.exc import IntegrityError

from trading_replay_api.db.schema import (
    commands,
    commitments,
    domain_events,
    result_bundles,
    rulesets,
    sessions,
)

from .model import (
    FrozenResult,
    ResultErrorCode,
    ResultServiceError,
    canonical_hash,
    mapping,
)


@dataclass(frozen=True, slots=True)
class AuthoritativeResultData:
    """Rows that must agree with completion evidence before a result can freeze."""

    ruleset: dict[str, object]
    commands: tuple[dict[str, object], ...]
    domain_events: tuple[dict[str, object], ...]
    commitments: tuple[dict[str, object], ...]


class ResultStore:
    """Read persisted replay facts and freeze one immutable export per session."""

    def __init__(self, engine: Engine) -> None:
        self.engine = engine

    def authoritative(
        self,
        *,
        session_id: str,
        principal_id: str,
        ruleset_id: str,
    ) -> AuthoritativeResultData:
        """Return principal-scoped persisted facts in deterministic order."""
        with self.engine.connect() as connection:
            self._require_principal(connection, session_id, principal_id)
            ruleset_row = connection.execute(
                select(
                    rulesets.c.ruleset_id,
                    rulesets.c.ruleset_version,
                    rulesets.c.ruleset_hash,
                    rulesets.c.body_json,
                ).where(rulesets.c.ruleset_id == ruleset_id)
            ).one_or_none()
            if ruleset_row is None:
                raise ResultServiceError(
                    ResultErrorCode.PERSISTED_CONFLICT,
                    "completed session references a missing ruleset",
                )
            ruleset = {
                "ruleset_id": str(ruleset_row.ruleset_id),
                "ruleset_version": str(ruleset_row.ruleset_version),
                "ruleset_hash": str(ruleset_row.ruleset_hash),
                "body": dict(mapping(ruleset_row.body_json, "stored ruleset body")),
            }

            command_rows = connection.execute(
                select(
                    commands.c.command_id,
                    commands.c.idempotency_key,
                    commands.c.payload_hash,
                    commands.c.expected_session_version,
                    commands.c.accepted_at_ns,
                    commands.c.payload_json,
                )
                .where(commands.c.session_id == session_id)
                .order_by(commands.c.expected_session_version, commands.c.command_id)
            )
            command_values = tuple(
                {
                    "command_id": str(row.command_id),
                    "idempotency_key": str(row.idempotency_key),
                    "payload_hash": str(row.payload_hash),
                    "expected_session_version": int(row.expected_session_version),
                    "accepted_at_ns": int(row.accepted_at_ns),
                    "payload": dict(mapping(row.payload_json, "stored command payload")),
                }
                for row in command_rows
            )

            event_rows = connection.execute(
                select(
                    domain_events.c.event_seq,
                    domain_events.c.logical_ts_ns,
                    domain_events.c.event_type,
                    domain_events.c.causation_id,
                    domain_events.c.correlation_id,
                    domain_events.c.payload_json,
                    domain_events.c.prior_event_hash,
                    domain_events.c.current_event_hash,
                )
                .where(domain_events.c.session_id == session_id)
                .order_by(domain_events.c.event_seq)
            )
            event_values = tuple(
                {
                    "event_seq": int(row.event_seq),
                    "logical_ts_ns": int(row.logical_ts_ns),
                    "event_type": str(row.event_type),
                    "causation_id": str(row.causation_id),
                    "correlation_id": str(row.correlation_id),
                    "payload": dict(mapping(row.payload_json, "stored domain-event payload")),
                    "prior_event_hash": str(row.prior_event_hash),
                    "current_event_hash": str(row.current_event_hash),
                }
                for row in event_rows
            )

            commitment_rows = connection.execute(
                select(
                    commitments.c.commitment_id,
                    commitments.c.kind,
                    commitments.c.algorithm_version,
                    commitments.c.commitment_hash,
                    commitments.c.setup_hash,
                    commitments.c.eligible_set_hash,
                    commitments.c.metadata_json,
                    commitments.c.revealed_secret,
                )
                .where(commitments.c.session_id == session_id)
                .order_by(commitments.c.kind, commitments.c.commitment_id)
            )
            commitment_values = tuple(
                {
                    "commitment_id": str(row.commitment_id),
                    "kind": str(row.kind),
                    "algorithm_version": str(row.algorithm_version),
                    "commitment_hash": str(row.commitment_hash),
                    "setup_hash": str(row.setup_hash),
                    "eligible_set_hash": str(row.eligible_set_hash),
                    "metadata": dict(mapping(row.metadata_json, "stored commitment metadata")),
                    "revealed_secret": (
                        None if row.revealed_secret is None else str(row.revealed_secret)
                    ),
                }
                for row in commitment_rows
            )
            return AuthoritativeResultData(
                ruleset=ruleset,
                commands=command_values,
                domain_events=event_values,
                commitments=commitment_values,
            )

    def freeze(self, result: FrozenResult, *, principal_id: str) -> FrozenResult:
        """Persist once, accepting only a byte-identical idempotent retry."""
        try:
            with self.engine.begin() as connection:
                self._require_principal(connection, result.session_id, principal_id)
                existing = connection.execute(
                    select(result_bundles).where(result_bundles.c.session_id == result.session_id)
                ).one_or_none()
                if existing is not None:
                    materialized = self._materialize(existing, replayed=True)
                    if materialized.export_hash != result.export_hash:
                        raise ResultServiceError(
                            ResultErrorCode.RESULT_CONFLICT,
                            "session already has a different immutable result",
                        )
                    return materialized
                connection.execute(
                    insert(result_bundles).values(
                        session_id=result.session_id,
                        result_hash=result.result_hash,
                        bundle_hash=result.bundle_hash,
                        proof_hash=result.proof_hash,
                        export_hash=result.export_hash,
                        created_at_ns=result.created_at_ns,
                        bundle_json=result.bundle,
                        proof_json=result.proof,
                        export_json=result.export,
                    )
                )
                return result
        except IntegrityError as error:
            raise ResultServiceError(
                ResultErrorCode.DATABASE_CONFLICT,
                "concurrent result finalization conflicted",
            ) from error

    def get(self, *, session_id: str, principal_id: str) -> FrozenResult:
        """Return a frozen result after rechecking its stored content hashes."""
        with self.engine.connect() as connection:
            self._require_principal(connection, session_id, principal_id)
            row = connection.execute(
                select(result_bundles).where(result_bundles.c.session_id == session_id)
            ).one_or_none()
            if row is None:
                raise ResultServiceError(ResultErrorCode.RESULT_NOT_FOUND, "result does not exist")
            return self._materialize(row, replayed=False)

    @staticmethod
    def _require_principal(connection: Any, session_id: str, principal_id: str) -> None:
        owner = connection.execute(
            select(sessions.c.principal_id).where(sessions.c.session_id == session_id)
        ).scalar_one_or_none()
        if owner is None or str(owner) != principal_id:
            raise ResultServiceError(
                ResultErrorCode.SESSION_UNAVAILABLE,
                "session is unavailable",
            )

    @staticmethod
    def _materialize(row: Any, *, replayed: bool) -> FrozenResult:
        bundle = dict(mapping(row.bundle_json, "stored result bundle"))
        proof = dict(mapping(row.proof_json, "stored verifier proof"))
        export = dict(mapping(row.export_json, "stored result export"))
        bundle_hash = str(row.bundle_hash)
        proof_hash = str(row.proof_hash)
        export_hash = str(row.export_hash)
        if (
            canonical_hash(bundle) != bundle_hash
            or canonical_hash(proof) != proof_hash
            or canonical_hash(export) != export_hash
        ):
            raise ResultServiceError(
                ResultErrorCode.PERSISTED_CONFLICT,
                "stored result content does not match its immutable hash",
            )
        if str(row.result_hash) != str(bundle.get("result_hash")):
            raise ResultServiceError(
                ResultErrorCode.PERSISTED_CONFLICT,
                "stored result hash disagrees with result bundle",
            )
        return FrozenResult(
            session_id=str(row.session_id),
            result_hash=str(row.result_hash),
            bundle_hash=bundle_hash,
            proof_hash=proof_hash,
            export_hash=export_hash,
            created_at_ns=int(row.created_at_ns),
            bundle=bundle,
            proof=proof,
            export=export,
            replayed=replayed,
        )


__all__ = ["AuthoritativeResultData", "ResultStore"]
