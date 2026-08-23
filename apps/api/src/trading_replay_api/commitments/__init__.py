"""Pre-selection commitments, unbiased random selection, reveal, and offline verification."""

from __future__ import annotations

import base64
import binascii
import hashlib
import hmac
import secrets
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import cast

from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from sqlalchemy import Connection, Engine, insert, select, update
from sqlalchemy.exc import IntegrityError

from trading_replay_api.db.schema import commitments, sessions

ALGORITHM_VERSION = "trl-episode-v1"
_KIND = "EPISODE_SELECTION"
_I64_MIN = -(2**63)
_I64_MAX = 2**63 - 1
_U64_MAX = 2**64 - 1
_MAX_PLAYER_NONCE_BYTES = 256


class CommitmentError(RuntimeError):
    """Base class for stable commitment failures."""


class CommitmentNotFound(CommitmentError):
    """Requested commitment does not exist for the authorized principal."""


class CommitmentExists(CommitmentError):
    """A session already has an episode-selection commitment."""


class CommitmentPrincipalMismatch(CommitmentError):
    """Session belongs to another principal."""


class CommitmentStateError(CommitmentError):
    """Session lifecycle does not permit the requested commitment operation."""


class CommitmentVerificationError(CommitmentError):
    """Commitment/proof inputs fail canonical or cryptographic verification."""


@dataclass(frozen=True, slots=True)
class SelectionSetup:
    """Selection-affecting setup fields committed before an episode is drawn."""

    instrument_id: str
    ruleset_hash: str
    execution_tier: str
    warmup_ns: int
    duration_ns: int
    visibility_mode: str
    required_capabilities: tuple[str, ...] = ()
    allowed_redistribution: tuple[str, ...] = ()
    allow_degraded: bool = False

    def __post_init__(self) -> None:
        if not self.instrument_id or not self.execution_tier or not self.visibility_mode:
            raise ValueError("selection setup string fields cannot be empty")
        _validate_sha256(self.ruleset_hash, "ruleset_hash")
        _validate_i64(self.warmup_ns, "warmup_ns")
        _validate_i64(self.duration_ns, "duration_ns")
        if self.warmup_ns < 0 or self.duration_ns <= 0:
            raise ValueError("warmup must be non-negative and duration must be positive")
        _require_canonical_strings(self.required_capabilities, "required_capabilities")
        _require_canonical_strings(self.allowed_redistribution, "allowed_redistribution")


@dataclass(frozen=True, slots=True)
class EligibleEpisode:
    """One canonically ordered candidate episode."""

    episode_id: str
    manifest_hash: str
    play_start_ns: int
    play_end_ns: int

    def __post_init__(self) -> None:
        if not self.episode_id:
            raise ValueError("episode_id cannot be empty")
        _validate_sha256(self.manifest_hash, "manifest_hash")
        _validate_i64(self.play_start_ns, "play_start_ns")
        _validate_i64(self.play_end_ns, "play_end_ns")
        if self.play_end_ns <= self.play_start_ns:
            raise ValueError("episode end must be after start")


@dataclass(frozen=True, slots=True)
class PublicCommitment:
    """Player-safe commitment metadata available before completion."""

    commitment_id: str
    session_id: str
    algorithm_version: str
    commitment_hash: str
    setup_hash: str
    eligible_set_hash: str
    revealed_secret_hex: str | None


@dataclass(frozen=True, slots=True)
class PreparedSelection:
    """Server-side result of committing first and then deriving the episode draw."""

    public_commitment: PublicCommitment
    selected_episode: EligibleEpisode


@dataclass(frozen=True, slots=True)
class CompletionProof:
    """Portable proof inputs revealed only after session completion."""

    algorithm_version: str
    commitment_hash: str
    setup_hash: str
    eligible_set_hash: str
    secret_hex: str
    player_nonce_hex: str
    selected_index: int
    draw_counter: int
    selected_episode: EligibleEpisode


@dataclass(frozen=True, slots=True)
class _SelectionResult:
    setup_hash: str
    eligible_set_hash: str
    commitment_hash: str
    selected_index: int
    draw_counter: int


def setup_hash(setup: SelectionSetup) -> str:
    """Return the domain-separated hash of canonical setup bytes."""
    return hashlib.sha256(_canonical_setup_bytes(setup)).hexdigest()


def eligible_set_hash(episodes: Sequence[EligibleEpisode]) -> str:
    """Return the hash of a strictly canonical ordered eligible episode list."""
    return hashlib.sha256(_canonical_eligible_bytes(episodes)).hexdigest()


def derive_selection(
    setup: SelectionSetup,
    episodes: Sequence[EligibleEpisode],
    *,
    secret: bytes,
    player_nonce: bytes = b"",
) -> _SelectionResult:
    """Commit to inputs and derive an unbiased deterministic selection.

    The eligible list must already be in strictly increasing `episode_id` order. This makes
    order part of the canonical protocol: reordering candidates is rejected rather than
    silently normalized. Selection uses HMAC-SHA256 rejection sampling, so no modulo bias or
    variable-width integer encoding enters the draw.
    """
    if len(secret) != 32:
        raise ValueError("selection secret must be exactly 32 bytes")
    _validate_player_nonce(player_nonce)
    if not episodes:
        raise ValueError("eligible episode list cannot be empty")

    setup_digest = setup_hash(setup)
    eligible_digest = eligible_set_hash(episodes)
    commitment_digest = _commitment_hash(secret, setup_digest, eligible_digest, player_nonce)
    selected_index, counter = _draw_index(
        secret,
        setup_digest,
        eligible_digest,
        player_nonce,
        len(episodes),
    )
    return _SelectionResult(
        setup_hash=setup_digest,
        eligible_set_hash=eligible_digest,
        commitment_hash=commitment_digest,
        selected_index=selected_index,
        draw_counter=counter,
    )


def verify_completion_proof(
    setup: SelectionSetup,
    episodes: Sequence[EligibleEpisode],
    proof: CompletionProof,
) -> EligibleEpisode:
    """Verify a completion proof and return its selected canonical episode.

    # Raises
    Raises [`CommitmentVerificationError`] for any changed setup, candidate order/content,
    secret, nonce, selection counter/index, algorithm version, or selected episode.
    """
    if proof.algorithm_version != ALGORITHM_VERSION:
        raise CommitmentVerificationError("unsupported commitment algorithm version")
    try:
        secret = bytes.fromhex(proof.secret_hex)
        nonce = bytes.fromhex(proof.player_nonce_hex)
        derived = derive_selection(setup, episodes, secret=secret, player_nonce=nonce)
    except (ValueError, TypeError) as error:
        raise CommitmentVerificationError("invalid completion proof encoding") from error

    if not hmac.compare_digest(derived.setup_hash, proof.setup_hash):
        raise CommitmentVerificationError("setup hash mismatch")
    if not hmac.compare_digest(derived.eligible_set_hash, proof.eligible_set_hash):
        raise CommitmentVerificationError("eligible set hash mismatch")
    if not hmac.compare_digest(derived.commitment_hash, proof.commitment_hash):
        raise CommitmentVerificationError("commitment hash mismatch")
    if derived.selected_index != proof.selected_index or derived.draw_counter != proof.draw_counter:
        raise CommitmentVerificationError("selection draw mismatch")
    if proof.selected_index < 0 or proof.selected_index >= len(episodes):
        raise CommitmentVerificationError("selected index is out of range")
    selected = episodes[proof.selected_index]
    if selected != proof.selected_episode:
        raise CommitmentVerificationError("selected episode mismatch")
    return selected


class CommitmentService:
    """Principal-scoped persistent pre-selection commitment service."""

    def __init__(
        self,
        engine: Engine,
        master_key: bytes,
        *,
        entropy: Callable[[int], bytes] = secrets.token_bytes,
    ) -> None:
        if len(master_key) != 32:
            raise ValueError("commitment master key must be exactly 32 bytes")
        self._engine = engine
        self._cipher = AESGCM(master_key)
        self._entropy = entropy

    def prepare_episode_selection(
        self,
        *,
        commitment_id: str,
        session_id: str,
        principal_id: str,
        setup: SelectionSetup,
        episodes: Sequence[EligibleEpisode],
        player_nonce: bytes = b"",
    ) -> PreparedSelection:
        """Persist a secret commitment before returning the selected episode to the caller."""
        if not commitment_id or not session_id or not principal_id:
            raise ValueError("commitment, session, and principal identifiers are required")
        secret = self._entropy(32)
        if len(secret) != 32:
            raise RuntimeError("entropy source returned an invalid selection secret")
        result = derive_selection(setup, episodes, secret=secret, player_nonce=player_nonce)
        selected = episodes[result.selected_index]
        sealed_secret = self._seal_secret(session_id, commitment_id, secret)
        metadata = {
            "draw_counter": str(result.draw_counter),
            "player_nonce_hex": player_nonce.hex(),
            "selected_episode_id": selected.episode_id,
            "selected_index": str(result.selected_index),
            "selected_manifest_hash": selected.manifest_hash,
            "selected_play_end_ns": str(selected.play_end_ns),
            "selected_play_start_ns": str(selected.play_start_ns),
        }

        try:
            with self._engine.begin() as connection:
                self._require_session(connection, session_id, principal_id, expected_status="SETUP")
                connection.execute(
                    insert(commitments).values(
                        commitment_id=commitment_id,
                        session_id=session_id,
                        kind=_KIND,
                        algorithm_version=ALGORITHM_VERSION,
                        commitment_hash=result.commitment_hash,
                        setup_hash=result.setup_hash,
                        eligible_set_hash=result.eligible_set_hash,
                        metadata_json=metadata,
                        sealed_secret=sealed_secret,
                        revealed_secret=None,
                    )
                )
        except IntegrityError as error:
            raise CommitmentExists(session_id) from error

        return PreparedSelection(
            public_commitment=PublicCommitment(
                commitment_id=commitment_id,
                session_id=session_id,
                algorithm_version=ALGORITHM_VERSION,
                commitment_hash=result.commitment_hash,
                setup_hash=result.setup_hash,
                eligible_set_hash=result.eligible_set_hash,
                revealed_secret_hex=None,
            ),
            selected_episode=selected,
        )

    def get_public(self, *, session_id: str, principal_id: str) -> PublicCommitment:
        """Return player-safe commitment metadata with no secret before reveal."""
        with self._engine.connect() as connection:
            self._require_session(connection, session_id, principal_id)
            row = connection.execute(
                select(
                    commitments.c.commitment_id,
                    commitments.c.algorithm_version,
                    commitments.c.commitment_hash,
                    commitments.c.setup_hash,
                    commitments.c.eligible_set_hash,
                    commitments.c.revealed_secret,
                ).where(
                    commitments.c.session_id == session_id,
                    commitments.c.kind == _KIND,
                )
            ).one_or_none()
            if row is None:
                raise CommitmentNotFound(session_id)
            return PublicCommitment(
                commitment_id=str(row[0]),
                session_id=session_id,
                algorithm_version=str(row[1]),
                commitment_hash=str(row[2]),
                setup_hash=str(row[3]),
                eligible_set_hash=str(row[4]),
                revealed_secret_hex=None if row[5] is None else str(row[5]),
            )

    def reveal_completed(self, *, session_id: str, principal_id: str) -> CompletionProof:
        """Reveal and persist the selection secret only after the session is completed."""
        with self._engine.begin() as connection:
            self._require_session(connection, session_id, principal_id, expected_status="COMPLETED")
            row = connection.execute(
                select(
                    commitments.c.commitment_id,
                    commitments.c.algorithm_version,
                    commitments.c.commitment_hash,
                    commitments.c.setup_hash,
                    commitments.c.eligible_set_hash,
                    commitments.c.metadata_json,
                    commitments.c.sealed_secret,
                    commitments.c.revealed_secret,
                ).where(
                    commitments.c.session_id == session_id,
                    commitments.c.kind == _KIND,
                )
            ).one_or_none()
            if row is None:
                raise CommitmentNotFound(session_id)

            commitment_id = str(row[0])
            algorithm_version = str(row[1])
            if algorithm_version != ALGORITHM_VERSION:
                raise CommitmentVerificationError("unsupported stored algorithm version")
            secret = self._unseal_secret(session_id, commitment_id, str(row[6]))
            secret_hex = secret.hex()
            if row[7] is not None and not hmac.compare_digest(str(row[7]), secret_hex):
                raise CommitmentVerificationError(
                    "stored revealed secret conflicts with sealed secret"
                )

            metadata = _metadata(row[5])
            player_nonce_hex = _metadata_string(metadata, "player_nonce_hex")
            try:
                player_nonce = bytes.fromhex(player_nonce_hex)
            except ValueError as error:
                raise CommitmentVerificationError("stored player nonce is invalid") from error
            setup_digest = str(row[3])
            eligible_digest = str(row[4])
            stored_commitment = str(row[2])
            recomputed = _commitment_hash(secret, setup_digest, eligible_digest, player_nonce)
            if not hmac.compare_digest(stored_commitment, recomputed):
                raise CommitmentVerificationError("sealed secret does not match commitment")

            if row[7] is None:
                connection.execute(
                    update(commitments)
                    .where(commitments.c.commitment_id == commitment_id)
                    .values(revealed_secret=secret_hex)
                )

            selected_episode = EligibleEpisode(
                episode_id=_metadata_string(metadata, "selected_episode_id"),
                manifest_hash=_metadata_string(metadata, "selected_manifest_hash"),
                play_start_ns=_metadata_int(metadata, "selected_play_start_ns"),
                play_end_ns=_metadata_int(metadata, "selected_play_end_ns"),
            )
            return CompletionProof(
                algorithm_version=algorithm_version,
                commitment_hash=stored_commitment,
                setup_hash=setup_digest,
                eligible_set_hash=eligible_digest,
                secret_hex=secret_hex,
                player_nonce_hex=player_nonce_hex,
                selected_index=_metadata_u64(metadata, "selected_index"),
                draw_counter=_metadata_u64(metadata, "draw_counter"),
                selected_episode=selected_episode,
            )

    def _seal_secret(self, session_id: str, commitment_id: str, secret: bytes) -> str:
        nonce = self._entropy(12)
        if len(nonce) != 12:
            raise RuntimeError("entropy source returned an invalid AEAD nonce")
        ciphertext = self._cipher.encrypt(
            nonce, secret, _associated_data(session_id, commitment_id)
        )
        return base64.urlsafe_b64encode(nonce + ciphertext).decode("ascii")

    def _unseal_secret(self, session_id: str, commitment_id: str, sealed: str) -> bytes:
        try:
            payload = base64.b64decode(sealed.encode("ascii"), altchars=b"-_", validate=True)
        except (binascii.Error, ValueError, UnicodeEncodeError) as error:
            raise CommitmentVerificationError("sealed selection secret is invalid") from error
        if len(payload) < 13:
            raise CommitmentVerificationError("sealed selection secret is truncated")
        try:
            secret = self._cipher.decrypt(
                payload[:12],
                payload[12:],
                _associated_data(session_id, commitment_id),
            )
        except (InvalidTag, ValueError) as error:
            raise CommitmentVerificationError(
                "sealed selection secret authentication failed"
            ) from error
        if len(secret) != 32:
            raise CommitmentVerificationError("unsealed selection secret has invalid length")
        return secret

    @staticmethod
    def _require_session(
        connection: Connection,
        session_id: str,
        principal_id: str,
        *,
        expected_status: str | None = None,
    ) -> None:
        row = connection.execute(
            select(sessions.c.principal_id, sessions.c.status).where(
                sessions.c.session_id == session_id
            )
        ).one_or_none()
        if row is None:
            raise CommitmentNotFound(session_id)
        if str(row[0]) != principal_id:
            raise CommitmentPrincipalMismatch(session_id)
        if expected_status is not None and str(row[1]) != expected_status:
            raise CommitmentStateError(
                f"session must be {expected_status} for commitment operation, stored {row[1]}"
            )


def _canonical_setup_bytes(setup: SelectionSetup) -> bytes:
    output = bytearray(b"trl:episode-setup:v1")
    output.extend(_text(setup.instrument_id))
    output.extend(bytes.fromhex(setup.ruleset_hash))
    output.extend(_text(setup.execution_tier))
    output.extend(_i64(setup.warmup_ns))
    output.extend(_i64(setup.duration_ns))
    output.extend(_text(setup.visibility_mode))
    output.extend(_strings(setup.required_capabilities))
    output.extend(_strings(setup.allowed_redistribution))
    output.extend(b"\x01" if setup.allow_degraded else b"\x00")
    return bytes(output)


def _canonical_eligible_bytes(episodes: Sequence[EligibleEpisode]) -> bytes:
    if not episodes:
        raise ValueError("eligible episode list cannot be empty")
    identifiers = tuple(episode.episode_id for episode in episodes)
    if identifiers != tuple(sorted(identifiers)) or len(set(identifiers)) != len(identifiers):
        raise ValueError("eligible episodes must have unique ids in canonical ascending order")
    output = bytearray(b"trl:eligible-episodes:v1")
    output.extend(len(episodes).to_bytes(8, "big"))
    for episode in episodes:
        encoded = bytearray()
        encoded.extend(_text(episode.episode_id))
        encoded.extend(bytes.fromhex(episode.manifest_hash))
        encoded.extend(_i64(episode.play_start_ns))
        encoded.extend(_i64(episode.play_end_ns))
        output.extend(len(encoded).to_bytes(8, "big"))
        output.extend(encoded)
    return bytes(output)


def _commitment_hash(
    secret: bytes,
    setup_digest: str,
    eligible_digest: str,
    player_nonce: bytes,
) -> str:
    payload = bytearray(b"trl:episode-commitment:v1")
    payload.extend(secret)
    payload.extend(bytes.fromhex(setup_digest))
    payload.extend(bytes.fromhex(eligible_digest))
    payload.extend(len(player_nonce).to_bytes(8, "big"))
    payload.extend(player_nonce)
    return hashlib.sha256(payload).hexdigest()


def _draw_index(
    secret: bytes,
    setup_digest: str,
    eligible_digest: str,
    player_nonce: bytes,
    count: int,
) -> tuple[int, int]:
    if count <= 0:
        raise ValueError("selection count must be positive")
    space = 1 << 256
    limit = space - (space % count)
    counter = 0
    while counter < 2**64:
        message = bytearray(b"trl:episode-draw:v1")
        message.extend(bytes.fromhex(setup_digest))
        message.extend(bytes.fromhex(eligible_digest))
        message.extend(len(player_nonce).to_bytes(8, "big"))
        message.extend(player_nonce)
        message.extend(counter.to_bytes(8, "big"))
        candidate = int.from_bytes(hmac.new(secret, message, hashlib.sha256).digest(), "big")
        if candidate < limit:
            return candidate % count, counter
        counter += 1
    raise RuntimeError("selection rejection counter exhausted")


def _associated_data(session_id: str, commitment_id: str) -> bytes:
    return b"trl:sealed-episode-secret:v1" + _text(session_id) + _text(commitment_id)


def _text(value: str) -> bytes:
    if not value:
        raise ValueError("canonical text field cannot be empty")
    encoded = value.encode("utf-8")
    return len(encoded).to_bytes(8, "big") + encoded


def _strings(values: tuple[str, ...]) -> bytes:
    output = bytearray(len(values).to_bytes(8, "big"))
    for value in values:
        output.extend(_text(value))
    return bytes(output)


def _i64(value: int) -> bytes:
    _validate_i64(value, "canonical integer")
    return value.to_bytes(8, "big", signed=True)


def _validate_i64(value: int, name: str) -> None:
    if isinstance(value, bool) or value < _I64_MIN or value > _I64_MAX:
        raise ValueError(f"{name} must fit signed 64-bit integer")


def _validate_sha256(value: str, name: str) -> None:
    if len(value) != 64:
        raise ValueError(f"{name} must be a lowercase SHA-256 hex digest")
    try:
        decoded = bytes.fromhex(value)
    except ValueError as error:
        raise ValueError(f"{name} must be a lowercase SHA-256 hex digest") from error
    if decoded.hex() != value:
        raise ValueError(f"{name} must be a lowercase SHA-256 hex digest")


def _require_canonical_strings(values: tuple[str, ...], name: str) -> None:
    if any(not value for value in values):
        raise ValueError(f"{name} cannot contain empty values")
    if values != tuple(sorted(values)) or len(set(values)) != len(values):
        raise ValueError(f"{name} must be unique and in canonical ascending order")


def _validate_player_nonce(value: bytes) -> None:
    if len(value) > _MAX_PLAYER_NONCE_BYTES:
        raise ValueError("player nonce exceeds canonical length limit")


def _metadata(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise CommitmentVerificationError("stored commitment metadata is invalid")
    return cast(dict[str, object], value)


def _metadata_string(value: dict[str, object], key: str) -> str:
    item = value.get(key)
    if not isinstance(item, str):
        raise CommitmentVerificationError(f"stored commitment metadata field {key} is invalid")
    return item


def _metadata_int(value: dict[str, object], key: str) -> int:
    item = _metadata_string(value, key)
    if (
        not item
        or (item[0] == "-" and not item[1:].isdigit())
        or (item[0] != "-" and not item.isdigit())
    ):
        raise CommitmentVerificationError(f"stored commitment metadata field {key} is invalid")
    parsed = int(item)
    _validate_i64(parsed, key)
    return parsed


def _metadata_u64(value: dict[str, object], key: str) -> int:
    item = _metadata_string(value, key)
    if not item or not item.isdigit():
        raise CommitmentVerificationError(f"stored commitment metadata field {key} is invalid")
    parsed = int(item)
    if parsed > _U64_MAX:
        raise CommitmentVerificationError(f"stored commitment metadata field {key} is invalid")
    return parsed
