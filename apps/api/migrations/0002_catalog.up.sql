BEGIN;

CREATE TABLE catalog_manifests (
  manifest_hash char(64) PRIMARY KEY,
  manifest_id text NOT NULL,
  provider text NOT NULL,
  dataset text NOT NULL,
  venue_id text NOT NULL,
  instrument_id text NOT NULL,
  adapter_version text NOT NULL,
  canonical_content_hash char(64) NOT NULL,
  actual_start_ns bigint NOT NULL,
  actual_end_ns bigint NOT NULL,
  status text NOT NULL,
  redistribution_class text NOT NULL,
  execution_tier text NOT NULL,
  capabilities_json jsonb NOT NULL,
  known_gaps_json jsonb NOT NULL,
  quality_decisions_json jsonb NOT NULL,
  provenance text NOT NULL DEFAULT '',
  ingested_at_ns bigint NOT NULL,
  CONSTRAINT uq_catalog_manifest_version UNIQUE (manifest_id, manifest_hash),
  CONSTRAINT ck_catalog_manifest_interval CHECK (actual_end_ns > actual_start_ns)
);

CREATE TABLE catalog_revocations (
  manifest_hash char(64) PRIMARY KEY REFERENCES catalog_manifests(manifest_hash),
  revoked_at_ns bigint NOT NULL,
  reason text NOT NULL,
  CONSTRAINT ck_catalog_revocation_reason CHECK (length(reason) > 0)
);

CREATE INDEX ix_catalog_coverage_lookup
  ON catalog_manifests(instrument_id, actual_start_ns, actual_end_ns, execution_tier);
CREATE INDEX ix_catalog_manifest_versions
  ON catalog_manifests(manifest_id, ingested_at_ns, manifest_hash);

COMMIT;
