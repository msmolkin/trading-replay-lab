"""Tardis normalized historical dataset adapter."""

from .adapter import (
    TardisAdapter,
    TardisConfig,
    TardisTransport,
    UrlLibTardisTransport,
)
from .model import (
    ApiKeyProvider,
    TardisCapabilities,
    TardisCoverage,
    TardisDataType,
    TardisEntitlement,
    TardisError,
    TardisIntegrityError,
    TardisRequestError,
    TardisSchemaError,
    capabilities_for_interval,
)

__all__ = [
    "ApiKeyProvider",
    "TardisAdapter",
    "TardisCapabilities",
    "TardisConfig",
    "TardisCoverage",
    "TardisDataType",
    "TardisEntitlement",
    "TardisError",
    "TardisIntegrityError",
    "TardisRequestError",
    "TardisSchemaError",
    "TardisTransport",
    "UrlLibTardisTransport",
    "capabilities_for_interval",
]
