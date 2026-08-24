"""Injectable FastAPI routes for authenticated trading commands."""

from __future__ import annotations

from collections.abc import Mapping
from typing import cast

from fastapi import APIRouter, Body, Header, HTTPException, Request

from trading_replay_api.auth import AuthenticationError, Authenticator

from .model import AcceptedCommand, CommandErrorCode, CommandServiceError, canonical_u64_text
from .service import TradingCommandService


def build_command_router(
    *,
    service: TradingCommandService,
    authenticator: Authenticator,
) -> APIRouter:
    """Build command routes with explicit service/authentication dependencies."""
    router = APIRouter(prefix="/sessions/{session_id}", tags=["commands"])

    @router.post("/orders")
    def submit_order(
        session_id: str,
        request: Request,
        body: dict[str, object] = Body(...),
        idempotency_key: str = Header(..., alias="Idempotency-Key"),
        expected_version: str = Header(..., alias="Expected-Session-Version"),
    ) -> dict[str, object]:
        principal_id = _authenticate(authenticator, request)
        return _response(
            _invoke(
                service.submit_order,
                session_id=session_id,
                principal_id=principal_id,
                idempotency_key=idempotency_key,
                expected_session_version=_version(expected_version),
                request=cast(Mapping[str, object], body),
            )
        )

    @router.post("/orders/{order_id}/cancel")
    def cancel_order(
        session_id: str,
        order_id: str,
        request: Request,
        body: dict[str, object] = Body(default_factory=dict),
        idempotency_key: str = Header(..., alias="Idempotency-Key"),
        expected_version: str = Header(..., alias="Expected-Session-Version"),
    ) -> dict[str, object]:
        principal_id = _authenticate(authenticator, request)
        return _response(
            _invoke(
                service.cancel_order,
                session_id=session_id,
                principal_id=principal_id,
                idempotency_key=idempotency_key,
                expected_session_version=_version(expected_version),
                order_id=order_id,
                request=cast(Mapping[str, object], body),
            )
        )

    @router.post("/orders/{order_id}/replace")
    def replace_order(
        session_id: str,
        order_id: str,
        request: Request,
        body: dict[str, object] = Body(...),
        idempotency_key: str = Header(..., alias="Idempotency-Key"),
        expected_version: str = Header(..., alias="Expected-Session-Version"),
    ) -> dict[str, object]:
        principal_id = _authenticate(authenticator, request)
        return _response(
            _invoke(
                service.replace_order,
                session_id=session_id,
                principal_id=principal_id,
                idempotency_key=idempotency_key,
                expected_session_version=_version(expected_version),
                order_id=order_id,
                request=cast(Mapping[str, object], body),
            )
        )

    @router.post("/leverage")
    def set_leverage(
        session_id: str,
        request: Request,
        body: dict[str, object] = Body(...),
        idempotency_key: str = Header(..., alias="Idempotency-Key"),
        expected_version: str = Header(..., alias="Expected-Session-Version"),
    ) -> dict[str, object]:
        principal_id = _authenticate(authenticator, request)
        return _response(
            _invoke(
                service.set_leverage,
                session_id=session_id,
                principal_id=principal_id,
                idempotency_key=idempotency_key,
                expected_session_version=_version(expected_version),
                request=cast(Mapping[str, object], body),
            )
        )

    @router.get("/commands/{command_id}")
    def get_command(
        session_id: str,
        command_id: str,
        request: Request,
    ) -> dict[str, object]:
        principal_id = _authenticate(authenticator, request)
        return _response(
            _invoke(
                service.get_command,
                session_id=session_id,
                principal_id=principal_id,
                command_id=command_id,
            )
        )

    return router


def _authenticate(authenticator: Authenticator, request: Request) -> str:
    try:
        principal = authenticator.authenticate(request.headers)
    except AuthenticationError as error:
        raise HTTPException(status_code=401, detail="authentication required") from error
    return principal.principal_id


def _version(raw: str) -> int:
    try:
        canonical = canonical_u64_text(raw, "Expected-Session-Version")
    except CommandServiceError as error:
        raise _http_error(error) from error
    return int(canonical)


def _invoke(function: object, /, **kwargs: object) -> AcceptedCommand:
    try:
        callable_function = cast(object, function)
        if not callable(callable_function):
            raise RuntimeError("command service method is not callable")
        result = callable_function(**kwargs)
        if not isinstance(result, AcceptedCommand):
            raise RuntimeError("command service returned unexpected result")
        return result
    except CommandServiceError as error:
        raise _http_error(error) from error


def _http_error(error: CommandServiceError) -> HTTPException:
    status = {
        CommandErrorCode.INVALID_COMMAND: 422,
        CommandErrorCode.SESSION_NOT_FOUND: 404,
        CommandErrorCode.PRINCIPAL_MISMATCH: 404,
        CommandErrorCode.SESSION_NOT_RUNNING: 409,
        CommandErrorCode.VERSION_CONFLICT: 409,
        CommandErrorCode.IDEMPOTENCY_CONFLICT: 409,
        CommandErrorCode.COMMAND_NOT_FOUND: 404,
        CommandErrorCode.QUOTE_UNAVAILABLE: 409,
        CommandErrorCode.DATABASE_CONFLICT: 409,
    }[error.code]
    return HTTPException(
        status_code=status,
        detail={"code": error.code.value, "message": str(error)},
    )


def _response(command: AcceptedCommand) -> dict[str, object]:
    return {
        "command_id": command.command_id,
        "session_id": command.session_id,
        "idempotency_key": command.idempotency_key,
        "payload_hash": command.payload_hash,
        "expected_session_version": str(command.expected_session_version),
        "resulting_session_version": str(command.resulting_session_version),
        "accepted_at_ns": str(command.accepted_at_ns),
        "payload": command.payload,
        "replayed": command.replayed,
    }


__all__ = ["build_command_router"]
