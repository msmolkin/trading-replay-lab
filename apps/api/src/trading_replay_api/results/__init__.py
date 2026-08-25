"""Immutable replay results and offline-verification exports."""

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
    StateHashEvidence,
    canonical_hash,
    canonical_json,
)
from .router import build_result_router
from .service import ResultService
from .store import ResultStore

__all__ = [
    "CanonicalInput",
    "CommandReplayMetadata",
    "FrozenResult",
    "LedgerPostingEvidence",
    "LedgerTransactionEvidence",
    "ResultErrorCode",
    "ResultEvidence",
    "ResultMetrics",
    "ResultService",
    "ResultServiceError",
    "ResultStore",
    "StateHashEvidence",
    "build_result_router",
    "canonical_hash",
    "canonical_json",
]
