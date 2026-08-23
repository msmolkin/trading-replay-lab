"""Coverage catalog domain and persistence schema."""

from .model import (
    CoverageSegment,
    DataCapability,
    EligibleSetup,
    EligibleWindow,
    ExecutionTier,
    Gap,
    ManifestRecord,
    ManifestStatus,
    RedistributionClass,
    SetupRequirement,
)
from .schema import catalog_manifests, catalog_metadata, catalog_revocations
from .service import CoverageCatalog, ManifestHashConflict, UnknownManifest

__all__ = [
    "CoverageCatalog",
    "CoverageSegment",
    "DataCapability",
    "EligibleSetup",
    "EligibleWindow",
    "ExecutionTier",
    "Gap",
    "ManifestHashConflict",
    "ManifestRecord",
    "ManifestStatus",
    "RedistributionClass",
    "SetupRequirement",
    "UnknownManifest",
    "catalog_manifests",
    "catalog_metadata",
    "catalog_revocations",
]
