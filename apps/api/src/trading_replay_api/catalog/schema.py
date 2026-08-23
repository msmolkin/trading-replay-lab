"""SQLAlchemy Core schema for immutable catalog manifests and revocations."""

from __future__ import annotations

from sqlalchemy import (
    JSON,
    BigInteger,
    CheckConstraint,
    Column,
    ForeignKey,
    Index,
    MetaData,
    String,
    Table,
    Text,
    UniqueConstraint,
)

catalog_metadata = MetaData()

catalog_manifests = Table(
    "catalog_manifests",
    catalog_metadata,
    Column("manifest_hash", String(64), primary_key=True),
    Column("manifest_id", String(200), nullable=False),
    Column("provider", String(200), nullable=False),
    Column("dataset", String(200), nullable=False),
    Column("venue_id", String(200), nullable=False),
    Column("instrument_id", String(200), nullable=False),
    Column("adapter_version", String(128), nullable=False),
    Column("canonical_content_hash", String(64), nullable=False),
    Column("actual_start_ns", BigInteger, nullable=False),
    Column("actual_end_ns", BigInteger, nullable=False),
    Column("status", String(32), nullable=False),
    Column("redistribution_class", String(32), nullable=False),
    Column("execution_tier", String(8), nullable=False),
    Column("capabilities_json", JSON, nullable=False),
    Column("known_gaps_json", JSON, nullable=False),
    Column("quality_decisions_json", JSON, nullable=False),
    Column("provenance", Text, nullable=False, default=""),
    Column("ingested_at_ns", BigInteger, nullable=False),
    UniqueConstraint(
        "manifest_id",
        "manifest_hash",
        name="uq_catalog_manifest_version",
    ),
    CheckConstraint(
        "actual_end_ns > actual_start_ns",
        name="ck_catalog_manifest_interval",
    ),
)

catalog_revocations = Table(
    "catalog_revocations",
    catalog_metadata,
    Column(
        "manifest_hash",
        ForeignKey("catalog_manifests.manifest_hash"),
        primary_key=True,
    ),
    Column("revoked_at_ns", BigInteger, nullable=False),
    Column("reason", Text, nullable=False),
    CheckConstraint("length(reason) > 0", name="ck_catalog_revocation_reason"),
)

Index(
    "ix_catalog_coverage_lookup",
    catalog_manifests.c.instrument_id,
    catalog_manifests.c.actual_start_ns,
    catalog_manifests.c.actual_end_ns,
    catalog_manifests.c.execution_tier,
)
Index(
    "ix_catalog_manifest_versions",
    catalog_manifests.c.manifest_id,
    catalog_manifests.c.ingested_at_ns,
    catalog_manifests.c.manifest_hash,
)
