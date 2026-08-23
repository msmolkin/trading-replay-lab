BEGIN;

ALTER TABLE commitments DROP COLUMN sealed_secret;
ALTER TABLE commitments DROP COLUMN metadata_json;
ALTER TABLE commitments DROP COLUMN eligible_set_hash;
ALTER TABLE commitments DROP COLUMN setup_hash;
ALTER TABLE commitments DROP COLUMN algorithm_version;

COMMIT;
