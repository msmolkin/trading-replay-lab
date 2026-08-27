from __future__ import annotations

from dataclasses import dataclass, field

import pytest
from fastapi.routing import APIRoute
from sqlalchemy import create_engine, insert, select
from sqlalchemy.engine import Engine

from trading_replay_api.auth import LocalAuthenticator
from trading_replay_api.commands import (
    CommandErrorCode,
    CommandServiceError,
    CommandStore,
    TradingCommandService,
    VisibleQuote,
    build_command_router,
)
from trading_replay_api.db.schema import commands, metadata, sessions
from trading_replay_api.sessions import SessionStatus


@dataclass(frozen=True, slots=True)
class FixedClock:
    value: int = 123

    def now_ns(self) -> int:
        return self.value


@dataclass(frozen=True, slots=True)
class FixedQuoteResolver:
    quote: VisibleQuote | None = field(default_factory=lambda: VisibleQuote("quote-9", 100, 103))

    def current_quote(self, *, session_id: str, principal_id: str) -> VisibleQuote | None:
        assert session_id == "session-1"
        assert principal_id == "principal-1"
        return self.quote


def service() -> tuple[TradingCommandService, Engine]:
    engine = create_engine("sqlite+pysqlite:///:memory:")
    metadata.create_all(engine)
    with engine.begin() as connection:
        connection.execute(
            insert(sessions).values(
                session_id="session-1",
                principal_id="principal-1",
                status=SessionStatus.RUNNING.value,
                version=7,
                created_at_ns=0,
            )
        )
    return (
        TradingCommandService(
            store=CommandStore(engine),
            quote_resolver=FixedQuoteResolver(),
            clock=FixedClock(),
        ),
        engine,
    )


def order(
    *, quantity_atoms: object = "2", extra: dict[str, object] | None = None
) -> dict[str, object]:
    body: dict[str, object] = {
        "instrument_id": "SYNTH",
        "side": "BUY",
        "quantity_atoms": quantity_atoms,
        "order_type": "LIMIT",
        "price_reference": "MIDPOINT",
        "time_in_force": "GTC",
        "reduce_only": False,
        "post_only": False,
        "marketable_only": False,
    }
    if extra:
        body.update(extra)
    return body


def test_midpoint_shortcut_uses_visible_quote_and_exact_retry_is_stable() -> None:
    commands_service, engine = service()
    first = commands_service.submit_order(
        session_id="session-1",
        principal_id="principal-1",
        idempotency_key="order-1",
        expected_session_version=7,
        request=order(),
    )

    assert first.payload["limit_price_atoms"] == "101"
    assert first.payload["price_reference"] == "MIDPOINT"
    assert first.payload["quote_event_id"] == "quote-9"
    assert first.resulting_session_version == 8
    assert first.accepted_at_ns == 123
    assert not first.replayed

    retry = commands_service.submit_order(
        session_id="session-1",
        principal_id="principal-1",
        idempotency_key="order-1",
        expected_session_version=7,
        request=order(),
    )
    assert retry.command_id == first.command_id
    assert retry.payload_hash == first.payload_hash
    assert retry.resulting_session_version == 8
    assert retry.replayed

    with engine.connect() as connection:
        assert (
            int(
                connection.execute(
                    select(sessions.c.version).where(sessions.c.session_id == "session-1")
                ).scalar_one()
            )
            == 8
        )
        assert len(connection.execute(select(commands)).all()) == 1


def test_exact_replacement_persists_flat_canonical_payload() -> None:
    commands_service, _ = service()
    accepted = commands_service.replace_order(
        session_id="session-1",
        principal_id="principal-1",
        idempotency_key="replace-1",
        expected_session_version=7,
        order_id="order-9",
        request={
            "quantity_atoms": "5",
            "limit_price_atoms": "102",
            "time_in_force": "GTC",
            "reduce_only": False,
            "post_only": True,
            "marketable_only": False,
        },
    )

    assert accepted.payload == {
        "command_type": "REPLACE_ORDER",
        "order_id": "order-9",
        "quantity_atoms": "5",
        "limit_price_atoms": "102",
        "time_in_force": "GTC",
        "reduce_only": False,
        "post_only": True,
        "marketable_only": False,
    }
    assert accepted.resulting_session_version == 8


def test_replacement_quote_shortcut_fails_closed_without_state_change() -> None:
    commands_service, engine = service()

    with pytest.raises(CommandServiceError, match="unsupported command fields: price_reference") as caught:
        commands_service.replace_order(
            session_id="session-1",
            principal_id="principal-1",
            idempotency_key="replace-shortcut",
            expected_session_version=7,
            order_id="order-9",
            request={"price_reference": "MIDPOINT"},
        )
    assert caught.value.code is CommandErrorCode.INVALID_COMMAND

    with engine.connect() as connection:
        assert (
            int(
                connection.execute(
                    select(sessions.c.version).where(sessions.c.session_id == "session-1")
                ).scalar_one()
            )
            == 7
        )
        assert connection.execute(select(commands)).all() == []


def test_same_idempotency_key_with_changed_payload_conflicts() -> None:
    commands_service, _ = service()
    commands_service.submit_order(
        session_id="session-1",
        principal_id="principal-1",
        idempotency_key="same-key",
        expected_session_version=7,
        request=order(),
    )

    with pytest.raises(CommandServiceError) as caught:
        commands_service.submit_order(
            session_id="session-1",
            principal_id="principal-1",
            idempotency_key="same-key",
            expected_session_version=7,
            request=order(quantity_atoms="3"),
        )
    assert caught.value.code is CommandErrorCode.IDEMPOTENCY_CONFLICT


def test_stale_new_command_and_cross_principal_access_fail() -> None:
    commands_service, _ = service()
    accepted = commands_service.set_leverage(
        session_id="session-1",
        principal_id="principal-1",
        idempotency_key="lev-1",
        expected_session_version=7,
        request={"leverage": 3},
    )

    with pytest.raises(CommandServiceError) as stale:
        commands_service.cancel_order(
            session_id="session-1",
            principal_id="principal-1",
            idempotency_key="cancel-1",
            expected_session_version=7,
            order_id="order-1",
            request={},
        )
    assert stale.value.code is CommandErrorCode.VERSION_CONFLICT

    with pytest.raises(CommandServiceError) as cross_principal:
        commands_service.get_command(
            session_id="session-1",
            principal_id="other-principal",
            command_id=accepted.command_id,
        )
    assert cross_principal.value.code is CommandErrorCode.PRINCIPAL_MISMATCH


def test_authoritative_fields_float_money_and_conflicting_prices_fail_closed() -> None:
    commands_service, _ = service()

    with pytest.raises(CommandServiceError) as injected:
        commands_service.submit_order(
            session_id="session-1",
            principal_id="principal-1",
            idempotency_key="bad-field",
            expected_session_version=7,
            request=order(extra={"fee_minor": "100"}),
        )
    assert injected.value.code is CommandErrorCode.INVALID_COMMAND

    with pytest.raises(CommandServiceError, match="canonical unsigned"):
        commands_service.submit_order(
            session_id="session-1",
            principal_id="principal-1",
            idempotency_key="float-qty",
            expected_session_version=7,
            request=order(quantity_atoms=2.0),
        )

    with pytest.raises(CommandServiceError, match="either price_reference"):
        commands_service.submit_order(
            session_id="session-1",
            principal_id="principal-1",
            idempotency_key="two-prices",
            expected_session_version=7,
            request=order(extra={"limit_price_atoms": "101"}),
        )


def test_router_exposes_canonical_command_endpoints() -> None:
    commands_service, _ = service()
    router = build_command_router(
        service=commands_service,
        authenticator=LocalAuthenticator(principal_id="principal-1"),
    )
    paths = {route.path for route in router.routes if isinstance(route, APIRoute)}
    assert paths == {
        "/sessions/{session_id}/orders",
        "/sessions/{session_id}/orders/{order_id}/cancel",
        "/sessions/{session_id}/orders/{order_id}/replace",
        "/sessions/{session_id}/leverage",
        "/sessions/{session_id}/commands/{command_id}",
    }
