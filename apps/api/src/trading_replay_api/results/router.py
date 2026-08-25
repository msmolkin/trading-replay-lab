"""Read-only authenticated routes for immutable result downloads."""

from __future__ import annotations

from fastapi import APIRouter, HTTPException, Request, Response

from trading_replay_api.auth import AuthenticationError, Authenticator

from .model import FrozenResult, ResultErrorCode, ResultServiceError, canonical_json
from .service import ResultService


def build_result_router(*, service: ResultService, authenticator: Authenticator) -> APIRouter:
    """Build principal-scoped result routes; finalization is intentionally not public HTTP."""
    router = APIRouter(prefix="/sessions/{session_id}/result", tags=["results"])

    @router.get("")
    def result_bundle(session_id: str, request: Request) -> Response:
        principal_id = _authenticate(authenticator, request)
        result = _invoke(service, session_id, principal_id)
        return _json_download(
            result.bundle,
            content_hash=result.bundle_hash,
            filename=f"{_filename_token(session_id)}-result.json",
            attachment=False,
        )

    @router.get("/proof")
    def verifier_proof(session_id: str, request: Request) -> Response:
        principal_id = _authenticate(authenticator, request)
        result = _invoke(service, session_id, principal_id)
        return _json_download(
            result.proof,
            content_hash=result.proof_hash,
            filename=f"{_filename_token(session_id)}-proof.json",
            attachment=True,
        )

    @router.get("/export")
    def export_package(session_id: str, request: Request) -> Response:
        principal_id = _authenticate(authenticator, request)
        result = _invoke(service, session_id, principal_id)
        return _json_download(
            result.export,
            content_hash=result.export_hash,
            filename=f"{_filename_token(session_id)}-export.json",
            attachment=True,
        )

    return router


def _authenticate(authenticator: Authenticator, request: Request) -> str:
    try:
        principal = authenticator.authenticate(request.headers)
    except AuthenticationError as error:
        raise HTTPException(status_code=401, detail="authentication required") from error
    return principal.principal_id


def _invoke(service: ResultService, session_id: str, principal_id: str) -> FrozenResult:
    try:
        return service.get(session_id=session_id, principal_id=principal_id)
    except ResultServiceError as error:
        raise _http_error(error) from error


def _http_error(error: ResultServiceError) -> HTTPException:
    status = {
        ResultErrorCode.SESSION_UNAVAILABLE: 404,
        ResultErrorCode.SESSION_NOT_COMPLETED: 409,
        ResultErrorCode.INVALID_EVIDENCE: 422,
        ResultErrorCode.PERSISTED_CONFLICT: 409,
        ResultErrorCode.RESULT_CONFLICT: 409,
        ResultErrorCode.RESULT_NOT_FOUND: 404,
        ResultErrorCode.DATABASE_CONFLICT: 409,
    }[error.code]
    return HTTPException(
        status_code=status,
        detail={"code": error.code.value, "message": str(error)},
    )


def _json_download(
    value: object,
    *,
    content_hash: str,
    filename: str,
    attachment: bool,
) -> Response:
    disposition = "attachment" if attachment else "inline"
    return Response(
        content=canonical_json(value),
        media_type="application/json",
        headers={
            "Cache-Control": "private, immutable, max-age=31536000",
            "Content-Disposition": f'{disposition}; filename="{filename}"',
            "X-Content-SHA256": content_hash,
            "X-Content-Type-Options": "nosniff",
        },
    )


def _filename_token(session_id: str) -> str:
    return "".join(character if character.isalnum() or character in "-_" else "_" for character in session_id)[:80] or "session"


__all__ = ["build_result_router"]
