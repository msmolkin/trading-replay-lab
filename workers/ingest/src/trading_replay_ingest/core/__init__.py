"""Provider-neutral ingestion framework."""

from .budget import BudgetExceeded, BudgetGuard
from .cache import CacheCorruption, ContentAddressedCache
from .model import Adapter, FetchChunk, FetchPlan, FetchRequest, NormalizedBatch
from .runner import IngestionRunner, RunResult
from .state import JobCheckpoint, JobStateStore

__all__ = [
    "Adapter",
    "BudgetExceeded",
    "BudgetGuard",
    "CacheCorruption",
    "ContentAddressedCache",
    "FetchChunk",
    "FetchPlan",
    "FetchRequest",
    "IngestionRunner",
    "JobCheckpoint",
    "JobStateStore",
    "NormalizedBatch",
    "RunResult",
]
