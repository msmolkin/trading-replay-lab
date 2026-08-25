CREATE TABLE result_bundles (
    session_id VARCHAR(160) PRIMARY KEY REFERENCES sessions(session_id),
    result_hash VARCHAR(64) NOT NULL,
    bundle_hash VARCHAR(64) NOT NULL,
    proof_hash VARCHAR(64) NOT NULL,
    export_hash VARCHAR(64) NOT NULL,
    created_at_ns BIGINT NOT NULL,
    bundle_json JSON NOT NULL,
    proof_json JSON NOT NULL,
    export_json JSON NOT NULL
);
