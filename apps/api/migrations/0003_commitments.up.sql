BEGIN;

ALTER TABLE commitments ADD COLUMN algorithm_version text;
ALTER TABLE commitments ADD COLUMN setup_hash char(64);
ALTER TABLE commitments ADD COLUMN eligible_set_hash char(64);
ALTER TABLE commitments ADD COLUMN metadata_json jsonb;
ALTER TABLE commitments ADD COLUMN sealed_secret text;

UPDATE commitments
SET algorithm_version = 'legacy',
    setup_hash = repeat('0', 64),
    eligible_set_hash = repeat('0', 64),
    metadata_json = '{}'::jsonb,
    sealed_secret = ''
WHERE algorithm_version IS NULL;

ALTER TABLE commitments ALTER COLUMN algorithm_version SET NOT NULL;
ALTER TABLE commitments ALTER COLUMN setup_hash SET NOT NULL;
ALTER TABLE commitments ALTER COLUMN eligible_set_hash SET NOT NULL;
ALTER TABLE commitments ALTER COLUMN metadata_json SET NOT NULL;
ALTER TABLE commitments ALTER COLUMN sealed_secret SET NOT NULL;

COMMIT;
