from __future__ import annotations

from dataclasses import replace
from pathlib import Path

import pytest
from sqlalchemy import create_engine, insert, select, update

from trading_replay_api.commitments import (
    ALGORITHM_VERSION,
    CommitmentExists,
    CommitmentPrincipalMismatch,
    CommitmentService,
    CommitmentStateError,
    CommitmentVerificationError,
    CompletionProof,
    EligibleEpisode,
    SelectionSetup,
    derive_selection,
    verify_completion_proof,
)
from trading_replay_api.db.schema import commitments, metadata, sessions


def setup() -> SelectionSetup:
    return SelectionSetup(
        instrument_id="SYNTH-BTC-USD",
        ruleset_hash="a" * 64,
        execution_tier="F1",
        warmup_ns=60,
        duration_ns=600,
        visibility_mode="RELATIVE",
        required_capabilities=("BBO", "TRADES"),
        allowed_redistribution=("REDISTRIBUTABLE",),
        allow_degraded=False,
    )


def episodes() -> tuple[EligibleEpisode, ...]:
    return (
        EligibleEpisode("episode-a", "1" * 64, 1_000, 1_600),
        EligibleEpisode("episode-b", "2" * 64, 2_000, 2_600),
        EligibleEpisode("episode-c", "3" * 64, 3_000, 3_600),
    )


def proof_for(secret: bytes, nonce: bytes = b"player") -> CompletionProof:
    candidates = episodes()
    result = derive_selection(setup(), candidates, secret=secret, player_nonce=nonce)
    return CompletionProof(
        algorithm_version=ALGORITHM_VERSION,
        commitment_hash=result.commitment_hash,
        setup_hash=result.setup_hash,
        eligible_set_hash=result.eligible_set_hash,
        secret_hex=secret.hex(),
        player_nonce_hex=nonce.hex(),
        selected_index=result.selected_index,
        draw_counter=result.draw_counter,
        selected_episode=candidates[result.selected_index],
    )


def service(tmp_path: Path) -> tuple[CommitmentService, object]:
    engine = create_engine(f"sqlite+pysqlite:///{tmp_path / 'commitments.db'}")
    metadata.create_all(engine)
    with engine.begin() as connection:
        connection.execute(
            insert(sessions).values(
                session_id="session-1",
                principal_id="principal-1",
                status="SETUP",
                version=0,
                created_at_ns=1,
            )
        )

    def entropy(length: int) -> bytes:
        return bytes([length]) * length

    return CommitmentService(engine, b"k" * 32, entropy=entropy), engine


def test_selection_and_proof_are_reproducible() -> None:
    secret = bytes(range(32))
    first = derive_selection(setup(), episodes(), secret=secret, player_nonce=b"player")
    second = derive_selection(setup(), episodes(), secret=secret, player_nonce=b"player")
    assert first == second
    proof = proof_for(secret)
    assert verify_completion_proof(setup(), episodes(), proof) == proof.selected_episode


def test_changed_setup_secret_or_candidate_order_fails_verification() -> None:
    secret = bytes(range(32))
    proof = proof_for(secret)
    changed_setup = replace(setup(), duration_ns=601)
    with pytest.raises(CommitmentVerificationError):
        verify_completion_proof(changed_setup, episodes(), proof)

    changed_secret = replace(proof, secret_hex=(b"x" * 32).hex())
    with pytest.raises(CommitmentVerificationError):
        verify_completion_proof(setup(), episodes(), changed_secret)

    reordered = (episodes()[1], episodes()[0], episodes()[2])
    with pytest.raises(CommitmentVerificationError):
        verify_completion_proof(setup(), reordered, proof)


def test_noncanonical_setup_lists_and_episode_order_reject() -> None:
    with pytest.raises(ValueError):
        replace(setup(), required_capabilities=("TRADES", "BBO"))
    with pytest.raises(ValueError):
        derive_selection(setup(), tuple(reversed(episodes())), secret=b"s" * 32)


def test_service_commits_before_return_and_reveals_only_after_completion(tmp_path: Path) -> None:
    commitments_service, engine = service(tmp_path)
    prepared = commitments_service.prepare_episode_selection(
        commitment_id="commitment-1",
        session_id="session-1",
        principal_id="principal-1",
        setup=setup(),
        episodes=episodes(),
        player_nonce=b"player",
    )
    public = commitments_service.get_public(
        session_id="session-1",
        principal_id="principal-1",
    )
    assert public == prepared.public_commitment
    assert public.revealed_secret_hex is None

    with engine.connect() as connection:
        stored = connection.execute(
            select(commitments.c.revealed_secret, commitments.c.sealed_secret).where(
                commitments.c.commitment_id == "commitment-1"
            )
        ).one()
        assert stored[0] is None
        assert (bytes([32]) * 32).hex() not in str(stored[1])

    with pytest.raises(CommitmentStateError):
        commitments_service.reveal_completed(
            session_id="session-1",
            principal_id="principal-1",
        )

    with engine.begin() as connection:
        connection.execute(
            update(sessions)
            .where(sessions.c.session_id == "session-1")
            .values(status="COMPLETED")
        )
    proof = commitments_service.reveal_completed(
        session_id="session-1",
        principal_id="principal-1",
    )
    assert proof.selected_episode == prepared.selected_episode
    assert verify_completion_proof(setup(), episodes(), proof) == prepared.selected_episode
    assert commitments_service.get_public(
        session_id="session-1",
        principal_id="principal-1",
    ).revealed_secret_hex == proof.secret_hex


def test_duplicate_and_cross_principal_operations_fail(tmp_path: Path) -> None:
    commitments_service, _ = service(tmp_path)
    commitments_service.prepare_episode_selection(
        commitment_id="commitment-1",
        session_id="session-1",
        principal_id="principal-1",
        setup=setup(),
        episodes=episodes(),
    )
    with pytest.raises(CommitmentExists):
        commitments_service.prepare_episode_selection(
            commitment_id="commitment-2",
            session_id="session-1",
            principal_id="principal-1",
            setup=setup(),
            episodes=episodes(),
        )
    with pytest.raises(CommitmentPrincipalMismatch):
        commitments_service.get_public(session_id="session-1", principal_id="principal-2")


def test_tampered_sealed_secret_fails_closed(tmp_path: Path) -> None:
    commitments_service, engine = service(tmp_path)
    commitments_service.prepare_episode_selection(
        commitment_id="commitment-1",
        session_id="session-1",
        principal_id="principal-1",
        setup=setup(),
        episodes=episodes(),
    )
    with engine.begin() as connection:
        connection.execute(
            update(commitments)
            .where(commitments.c.commitment_id == "commitment-1")
            .values(sealed_secret="AAAA")
        )
        connection.execute(
            update(sessions)
            .where(sessions.c.session_id == "session-1")
            .values(status="COMPLETED")
        )
    with pytest.raises(CommitmentVerificationError):
        commitments_service.reveal_completed(
            session_id="session-1",
            principal_id="principal-1",
        )


def test_commitment_migration_pair_exists() -> None:
    api_root = Path(__file__).resolve().parents[1]
    up = api_root / "migrations" / "0003_commitments.up.sql"
    down = api_root / "migrations" / "0003_commitments.down.sql"
    assert "algorithm_version" in up.read_text(encoding="utf-8")
    assert "DROP COLUMN algorithm_version" in down.read_text(encoding="utf-8")
