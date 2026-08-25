"""Deterministic result finalization and offline-verifier proof construction."""

from __future__ import annotations

import hashlib
from collections.abc import Mapping, Sequence
from dataclasses import replace

from trading_replay_api.commitments import (
    ALGORITHM_VERSION,
    CompletionProof,
    EligibleEpisode,
    SelectionSetup,
    verify_completion_proof,
)
from trading_replay_api.sessions import SessionLifecycleError, SessionStatus

from .model import (
    CanonicalInput,
    CommandReplayMetadata,
    FrozenResult,
    LedgerPostingEvidence,
    LedgerTransactionEvidence,
    ResultErrorCode,
    ResultEvidence,
    ResultMetrics,
    ResultServiceError,
    SessionReader,
    StateHashEvidence,
    canonical_hash,
)
from .store import AuthoritativeResultData, ResultStore

SCHEMA_VERSION = "1.0.0"
VERIFICATION_VERSION = "1"
EXPORT_VERSION = "1"
ZERO_HASH = "0" * 64


class ResultService:
    """Freeze a completed replay into one public bundle and one offline proof."""

    def __init__(self, *, sessions: SessionReader, store: ResultStore) -> None:
        self.sessions = sessions
        self.store = store

    def finalize(
        self,
        *,
        session_id: str,
        principal_id: str,
        evidence: ResultEvidence,
        created_at_ns: int,
    ) -> FrozenResult:
        """Validate completion evidence and freeze a deterministic immutable result."""
        _i64(created_at_ns, "created_at_ns")
        session = self._completed_session(session_id, principal_id)
        setup = session.setup
        if setup is None:
            raise ResultServiceError(
                ResultErrorCode.PERSISTED_CONFLICT,
                "completed session lost its committed setup",
            )
        authoritative = self.store.authoritative(
            session_id=session_id,
            principal_id=principal_id,
            ruleset_id=setup.ruleset_id,
        )
        self._validate_ruleset(authoritative, setup.ruleset_version, setup.ruleset_hash)
        command_payloads = _commands(
            session_id,
            principal_id,
            authoritative.commands,
            evidence.command_metadata,
        )
        domain_payloads = _domain_events(session_id, authoritative.domain_events)
        commitments, revealed_nonces = _commitment_exports(
            authoritative.commitments,
            setup=setup,
            eligible_episodes=evidence.eligible_episodes,
        )
        proof = _proof(session_id, setup.manifest_hash, evidence)
        result_hash = _string_field(proof, "result_hash")
        bundle: dict[str, object] = {
            "schema_version": SCHEMA_VERSION,
            "session_id": session_id,
            "setup": setup.to_payload(),
            "ruleset": authoritative.ruleset,
            "commitments": commitments,
            "revealed_nonces": revealed_nonces,
            "manifest_hashes": [setup.manifest_hash],
            "commands": command_payloads,
            "domain_events": domain_payloads,
            "state_hashes": _state_hash_json(evidence.state_hashes),
            "metrics": _metrics_json(evidence.metrics),
            "result_hash": result_hash,
        }
        bundle_hash = canonical_hash(bundle)
        proof_hash = canonical_hash(proof)
        export: dict[str, object] = {
            "export_version": EXPORT_VERSION,
            "session_id": session_id,
            "result_hash": result_hash,
            "bundle_hash": bundle_hash,
            "proof_hash": proof_hash,
            "result_bundle": bundle,
            "verifier_proof": proof,
        }
        export_hash = canonical_hash(export)
        frozen = FrozenResult(
            session_id=session_id,
            result_hash=result_hash,
            bundle_hash=bundle_hash,
            proof_hash=proof_hash,
            export_hash=export_hash,
            created_at_ns=created_at_ns,
            bundle=bundle,
            proof=proof,
            export=export,
            replayed=False,
        )
        return self.store.freeze(frozen, principal_id=principal_id)

    def get(self, *, session_id: str, principal_id: str) -> FrozenResult:
        """Return the immutable result for one owned session."""
        return self.store.get(session_id=session_id, principal_id=principal_id)

    def _completed_session(self, session_id: str, principal_id: str):  # type: ignore[no-untyped-def]
        try:
            session = self.sessions.get_session(session_id=session_id, principal_id=principal_id)
        except SessionLifecycleError as error:
            raise ResultServiceError(
                ResultErrorCode.SESSION_UNAVAILABLE,
                "session is unavailable",
            ) from error
        if session.status is not SessionStatus.COMPLETED:
            raise ResultServiceError(
                ResultErrorCode.SESSION_NOT_COMPLETED,
                "result finalization requires a completed replay session",
            )
        return session

    @staticmethod
    def _validate_ruleset(
        authoritative: AuthoritativeResultData,
        expected_version: str,
        expected_hash: str,
    ) -> None:
        if (
            authoritative.ruleset.get("ruleset_version") != expected_version
            or authoritative.ruleset.get("ruleset_hash") != expected_hash
        ):
            raise ResultServiceError(
                ResultErrorCode.PERSISTED_CONFLICT,
                "persisted ruleset does not match committed setup",
            )


def _commands(
    session_id: str,
    principal_id: str,
    rows: Sequence[Mapping[str, object]],
    metadata: Sequence[CommandReplayMetadata],
) -> list[dict[str, object]]:
    by_id: dict[str, CommandReplayMetadata] = {}
    arrival_sequences: set[int] = set()
    for item in metadata:
        if item.command_id in by_id or item.arrival_seq in arrival_sequences:
            raise _invalid("command replay metadata must have unique command and arrival ids")
        by_id[item.command_id] = item
        arrival_sequences.add(item.arrival_seq)
    row_ids = {_string_field(row, "command_id") for row in rows}
    if row_ids != set(by_id):
        raise _invalid("command replay metadata must match persisted commands exactly")

    output: list[dict[str, object]] = []
    prior_arrival: int | None = None
    for row in rows:
        command_id = _string_field(row, "command_id")
        item = by_id[command_id]
        if prior_arrival is not None and item.arrival_seq <= prior_arrival:
            raise _invalid("command arrival_seq must increase in persisted command order")
        prior_arrival = item.arrival_seq
        payload = row.get("payload")
        if not isinstance(payload, Mapping) or any(not isinstance(key, str) for key in payload):
            raise ResultServiceError(
                ResultErrorCode.PERSISTED_CONFLICT,
                "stored command payload is invalid",
            )
        output.append(
            {
                "schema_version": SCHEMA_VERSION,
                "command_id": command_id,
                "idempotency_key": _string_field(row, "idempotency_key"),
                "session_id": session_id,
                "principal_id": principal_id,
                "accepted_at_ns": str(_int_field(row, "accepted_at_ns")),
                "logical_ts_ns": str(item.logical_ts_ns),
                "arrival_seq": str(item.arrival_seq),
                "expected_session_version": str(_int_field(row, "expected_session_version")),
                "payload": dict(payload),
                "payload_hash": _sha_field(row, "payload_hash"),
            }
        )
    return output


def _domain_events(
    session_id: str,
    rows: Sequence[Mapping[str, object]],
) -> list[dict[str, object]]:
    output: list[dict[str, object]] = []
    prior_seq: int | None = None
    prior_hash: str | None = None
    for row in rows:
        event_seq = _int_field(row, "event_seq")
        current_hash = _sha_field(row, "current_event_hash")
        declared_prior = _sha_field(row, "prior_event_hash")
        if prior_seq is not None and event_seq <= prior_seq:
            raise ResultServiceError(
                ResultErrorCode.PERSISTED_CONFLICT,
                "persisted domain-event sequence is not strictly increasing",
            )
        if prior_hash is not None and declared_prior != prior_hash:
            raise ResultServiceError(
                ResultErrorCode.PERSISTED_CONFLICT,
                "persisted domain-event hash chain is discontinuous",
            )
        prior_seq = event_seq
        prior_hash = current_hash
        payload = row.get("payload")
        if not isinstance(payload, Mapping) or any(not isinstance(key, str) for key in payload):
            raise ResultServiceError(
                ResultErrorCode.PERSISTED_CONFLICT,
                "persisted domain-event payload is invalid",
            )
        output.append(
            {
                "schema_version": SCHEMA_VERSION,
                "session_id": session_id,
                "event_seq": str(event_seq),
                "logical_ts_ns": str(_int_field(row, "logical_ts_ns")),
                "event_type": _string_field(row, "event_type"),
                "causation_id": _string_field(row, "causation_id"),
                "correlation_id": _string_field(row, "correlation_id"),
                "payload": dict(payload),
                "prior_event_hash": declared_prior,
                "current_event_hash": current_hash,
            }
        )
    return output


def _commitment_exports(
    rows: Sequence[Mapping[str, object]],
    *,
    setup: object,
    eligible_episodes: Sequence[EligibleEpisode],
) -> tuple[list[str], list[str]]:
    hashes: list[str] = []
    nonces: list[str] = []
    for row in rows:
        commitment_hash = _sha_field(row, "commitment_hash")
        hashes.append(commitment_hash)
        if _string_field(row, "kind") != "EPISODE_SELECTION":
            continue
        if _string_field(row, "algorithm_version") != ALGORITHM_VERSION:
            raise ResultServiceError(
                ResultErrorCode.PERSISTED_CONFLICT,
                "stored episode commitment uses an unsupported algorithm",
            )
        revealed_secret = row.get("revealed_secret")
        if not isinstance(revealed_secret, str) or len(revealed_secret) != 64:
            raise _invalid("episode-selection secret must be revealed before result finalization")
        metadata = row.get("metadata")
        if not isinstance(metadata, Mapping):
            raise ResultServiceError(
                ResultErrorCode.PERSISTED_CONFLICT,
                "stored commitment metadata is invalid",
            )
        nonce = _metadata_string(metadata, "player_nonce_hex")
        proof = CompletionProof(
            algorithm_version=ALGORITHM_VERSION,
            commitment_hash=commitment_hash,
            setup_hash=_sha_field(row, "setup_hash"),
            eligible_set_hash=_sha_field(row, "eligible_set_hash"),
            secret_hex=revealed_secret,
            player_nonce_hex=nonce,
            selected_index=_metadata_int(metadata, "selected_index"),
            draw_counter=_metadata_int(metadata, "draw_counter"),
            selected_episode=EligibleEpisode(
                episode_id=_metadata_string(metadata, "selected_episode_id"),
                manifest_hash=_metadata_sha(metadata, "selected_manifest_hash"),
                play_start_ns=_metadata_int(metadata, "selected_play_start_ns"),
                play_end_ns=_metadata_int(metadata, "selected_play_end_ns"),
            ),
        )
        if not eligible_episodes:
            raise _invalid("eligible episode list is required to verify selection commitment")
        selection_setup = _selection_setup(setup)
        try:
            verify_completion_proof(selection_setup, eligible_episodes, proof)
        except (ValueError, RuntimeError) as error:
            raise _invalid("episode-selection completion proof did not verify") from error
        nonces.append(nonce)
    return hashes, nonces


def _selection_setup(setup: object) -> SelectionSetup:
    required = (
        "instrument_id",
        "ruleset_hash",
        "execution_tier",
        "warmup_ns",
        "duration_ns",
        "visibility_mode",
        "required_capabilities",
        "allowed_redistribution",
        "allow_degraded",
    )
    if any(not hasattr(setup, name) for name in required):
        raise ResultServiceError(
            ResultErrorCode.PERSISTED_CONFLICT,
            "committed setup cannot produce selection proof inputs",
        )
    return SelectionSetup(
        instrument_id=str(getattr(setup, "instrument_id")),
        ruleset_hash=str(getattr(setup, "ruleset_hash")),
        execution_tier=getattr(setup, "execution_tier").value,
        warmup_ns=int(getattr(setup, "warmup_ns")),
        duration_ns=int(getattr(setup, "duration_ns")),
        visibility_mode=getattr(setup, "visibility_mode").value,
        required_capabilities=tuple(
            sorted(item.value for item in getattr(setup, "required_capabilities"))
        ),
        allowed_redistribution=tuple(
            sorted(item.value for item in getattr(setup, "allowed_redistribution"))
        ),
        allow_degraded=bool(getattr(setup, "allow_degraded")),
    )


def _proof(session_id: str, manifest_hash: str, evidence: ResultEvidence) -> dict[str, object]:
    manifest_hashes = [manifest_hash]
    manifest_set_hash = _manifest_commitment(manifest_hashes)
    kernel_events, final_event_hash = _kernel_events(session_id, evidence.inputs)
    state_hashes = _state_hash_json(evidence.state_hashes)
    state_hashes_hash = _state_hash_commitment(evidence.state_hashes)
    ledger_transactions = _ledger_json(evidence.ledger_transactions)
    ledger_hash = _ledger_commitment(evidence.ledger_transactions)
    metrics = _metrics_json(evidence.metrics)
    result_hash = _result_commitment(
        session_id=session_id,
        manifest_set_hash=manifest_set_hash,
        final_event_hash=final_event_hash,
        state_hashes_hash=state_hashes_hash,
        ledger_hash=ledger_hash,
        metrics=evidence.metrics,
    )
    return {
        "verification_version": VERIFICATION_VERSION,
        "session_id": session_id,
        "manifest_hashes": manifest_hashes,
        "manifest_set_hash": manifest_set_hash,
        "inputs": [_input_json(item) for item in evidence.inputs],
        "kernel_events": kernel_events,
        "state_hashes": state_hashes,
        "state_hashes_hash": state_hashes_hash,
        "ledger_transactions": ledger_transactions,
        "ledger_hash": ledger_hash,
        "metrics": metrics,
        "result_hash": result_hash,
    }


def _kernel_events(
    session_id: str,
    inputs: Sequence[CanonicalInput],
) -> tuple[list[dict[str, object]], str]:
    events: list[dict[str, object]] = []
    prior_hash = bytes(32)
    for index, item in enumerate(inputs):
        if item.session_id != session_id:
            raise _invalid("canonical input session_id differs from finalized session")
        if item.input_seq != index or item.expected_state_version != index:
            raise _invalid("canonical inputs must be contiguous from sequence/version zero")
        payload = bytes.fromhex(item.payload_hex)
        payload_hash = hashlib.sha256(payload).digest()
        state_version = index + 1
        canonical_input = _kernel_input_bytes(item, payload)
        event_bytes = (
            b"TRL-KERNEL-EVENT-v1\0"
            + prior_hash
            + _u64(index)
            + _u64(state_version)
            + _i64_bytes(item.logical_ts_ns)
            + _text(item.kind)
            + payload_hash
            + _bytes(canonical_input)
        )
        current_hash = hashlib.sha256(event_bytes).digest()
        events.append(
            {
                "event_seq": str(index),
                "state_version": str(state_version),
                "logical_ts_ns": str(item.logical_ts_ns),
                "kind": item.kind,
                "payload_hash": payload_hash.hex(),
                "prior_event_hash": prior_hash.hex(),
                "current_event_hash": current_hash.hex(),
            }
        )
        prior_hash = current_hash
    return events, prior_hash.hex()


def _kernel_input_bytes(item: CanonicalInput, payload: bytes) -> bytes:
    return (
        b"TRL-KERNEL-INPUT-v1\0"
        + _text(item.session_id)
        + _u64(item.input_seq)
        + _u64(item.expected_state_version)
        + _i64_bytes(item.logical_ts_ns)
        + _text(item.kind)
        + _bytes(payload)
    )


def _manifest_commitment(hashes: Sequence[str]) -> str:
    raw = bytearray(b"TRL-MANIFEST-SET-v1\0")
    ordered = sorted(hashes)
    raw.extend(_u64(len(ordered)))
    for item in ordered:
        raw.extend(bytes.fromhex(_validated_sha(item, "manifest hash")))
    return hashlib.sha256(raw).hexdigest()


def _state_hash_commitment(states: Sequence[StateHashEvidence]) -> str:
    raw = bytearray(b"TRL-STATE-HASHES-v1\0")
    raw.extend(_u64(len(states)))
    prior: int | None = None
    for state in states:
        if prior is not None and state.event_seq <= prior:
            raise _invalid("state hashes must be strictly ordered by event_seq")
        prior = state.event_seq
        raw.extend(_u64(state.event_seq))
        raw.extend(bytes.fromhex(state.hash))
    return hashlib.sha256(raw).hexdigest()


def _ledger_commitment(transactions: Sequence[LedgerTransactionEvidence]) -> str:
    raw = bytearray(b"TRL-LEDGER-PROOF-v1\0")
    raw.extend(_u64(len(transactions)))
    seen: set[str] = set()
    prior_seq: int | None = None
    for transaction in transactions:
        if transaction.transaction_id in seen:
            raise _invalid("ledger transaction_id must be unique")
        seen.add(transaction.transaction_id)
        if prior_seq is not None and transaction.event_seq < prior_seq:
            raise _invalid("ledger transactions must be ordered by event_seq")
        prior_seq = transaction.event_seq
        _require_balanced(transaction)
        raw.extend(_u64(transaction.event_seq))
        raw.extend(_text(transaction.transaction_id))
        postings = sorted(
            transaction.postings,
            key=lambda posting: (posting.account, posting.amount_minor, posting.currency),
        )
        raw.extend(_u64(len(postings)))
        for posting in postings:
            raw.extend(_text(posting.currency))
            raw.extend(_text(posting.account))
            raw.extend(_i64_bytes(posting.amount_minor))
    return hashlib.sha256(raw).hexdigest()


def _require_balanced(transaction: LedgerTransactionEvidence) -> None:
    totals: dict[str, int] = {}
    for posting in transaction.postings:
        totals[posting.currency] = totals.get(posting.currency, 0) + posting.amount_minor
    if any(amount != 0 for amount in totals.values()):
        raise _invalid(f"ledger transaction {transaction.transaction_id} is not balanced")


def _result_commitment(
    *,
    session_id: str,
    manifest_set_hash: str,
    final_event_hash: str,
    state_hashes_hash: str,
    ledger_hash: str,
    metrics: ResultMetrics,
) -> str:
    raw = bytearray(b"TRL-VERIFIER-RESULT-v1\0")
    raw.extend(_text(session_id))
    for value in (manifest_set_hash, final_event_hash, state_hashes_hash, ledger_hash):
        raw.extend(bytes.fromhex(value))
    raw.extend(_u64(1 if metrics.survived else 0))
    raw.extend(_i64_bytes(metrics.terminal_return_ppb))
    raw.extend(_i64_bytes(metrics.max_drawdown_ppb))
    raw.extend(_i64_bytes(metrics.peak_effective_leverage_ppb))
    raw.extend(_i64_bytes(metrics.benchmark_return_ppb))
    return hashlib.sha256(raw).hexdigest()


def _input_json(item: CanonicalInput) -> dict[str, object]:
    return {
        "session_id": item.session_id,
        "input_seq": str(item.input_seq),
        "expected_state_version": str(item.expected_state_version),
        "logical_ts_ns": str(item.logical_ts_ns),
        "kind": item.kind,
        "payload_hex": item.payload_hex,
    }


def _state_hash_json(states: Sequence[StateHashEvidence]) -> list[dict[str, object]]:
    return [{"event_seq": str(item.event_seq), "hash": item.hash} for item in states]


def _ledger_json(
    transactions: Sequence[LedgerTransactionEvidence],
) -> list[dict[str, object]]:
    return [
        {
            "event_seq": str(transaction.event_seq),
            "transaction_id": transaction.transaction_id,
            "postings": [
                {
                    "account": posting.account,
                    "amount_minor": str(posting.amount_minor),
                    "currency": posting.currency,
                }
                for posting in transaction.postings
            ],
        }
        for transaction in transactions
    ]


def _metrics_json(metrics: ResultMetrics) -> dict[str, object]:
    return {
        "survived": metrics.survived,
        "terminal_return_ppb": str(metrics.terminal_return_ppb),
        "max_drawdown_ppb": str(metrics.max_drawdown_ppb),
        "peak_effective_leverage_ppb": str(metrics.peak_effective_leverage_ppb),
        "benchmark_return_ppb": str(metrics.benchmark_return_ppb),
    }


def _u64(value: int) -> bytes:
    if value < 0 or value > 2**64 - 1:
        raise _invalid("canonical unsigned value exceeds u64")
    return value.to_bytes(8, "big", signed=False)


def _i64_bytes(value: int) -> bytes:
    _i64(value, "canonical signed value")
    return value.to_bytes(8, "big", signed=True)


def _bytes(value: bytes) -> bytes:
    return _u64(len(value)) + value


def _text(value: str) -> bytes:
    return _bytes(value.encode("utf-8"))


def _i64(value: int, name: str) -> None:
    if isinstance(value, bool) or not isinstance(value, int) or value < -(2**63) or value > 2**63 - 1:
        raise _invalid(f"{name} must fit signed 64-bit integer")


def _string_field(fields: Mapping[str, object], name: str) -> str:
    value = fields.get(name)
    if not isinstance(value, str) or not value:
        raise ResultServiceError(
            ResultErrorCode.PERSISTED_CONFLICT,
            f"stored {name} is invalid",
        )
    return value


def _int_field(fields: Mapping[str, object], name: str) -> int:
    value = fields.get(name)
    if isinstance(value, bool) or not isinstance(value, int):
        raise ResultServiceError(
            ResultErrorCode.PERSISTED_CONFLICT,
            f"stored {name} is invalid",
        )
    return value


def _sha_field(fields: Mapping[str, object], name: str) -> str:
    value = _string_field(fields, name)
    try:
        return _validated_sha(value, name)
    except ResultServiceError as error:
        raise ResultServiceError(ResultErrorCode.PERSISTED_CONFLICT, str(error)) from error


def _validated_sha(value: str, name: str) -> str:
    if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
        raise _invalid(f"{name} must be lowercase SHA-256 hex")
    return value


def _metadata_string(fields: Mapping[str, object], name: str) -> str:
    value = fields.get(name)
    if not isinstance(value, str):
        raise ResultServiceError(
            ResultErrorCode.PERSISTED_CONFLICT,
            f"stored commitment {name} is invalid",
        )
    return value


def _metadata_sha(fields: Mapping[str, object], name: str) -> str:
    value = _metadata_string(fields, name)
    try:
        return _validated_sha(value, name)
    except ResultServiceError as error:
        raise ResultServiceError(ResultErrorCode.PERSISTED_CONFLICT, str(error)) from error


def _metadata_int(fields: Mapping[str, object], name: str) -> int:
    value = _metadata_string(fields, name)
    if not value or value.startswith("+") or (value.startswith("0") and value != "0"):
        raise ResultServiceError(
            ResultErrorCode.PERSISTED_CONFLICT,
            f"stored commitment {name} is not canonical unsigned decimal text",
        )
    if not value.isascii() or not value.isdigit():
        raise ResultServiceError(
            ResultErrorCode.PERSISTED_CONFLICT,
            f"stored commitment {name} is not canonical unsigned decimal text",
        )
    parsed = int(value)
    if parsed > 2**64 - 1:
        raise ResultServiceError(
            ResultErrorCode.PERSISTED_CONFLICT,
            f"stored commitment {name} exceeds u64",
        )
    return parsed


def _invalid(message: str) -> ResultServiceError:
    return ResultServiceError(ResultErrorCode.INVALID_EVIDENCE, message)


__all__ = ["EXPORT_VERSION", "ResultService", "SCHEMA_VERSION", "VERIFICATION_VERSION"]
