BEGIN;

CREATE TABLE rulesets (
  ruleset_id text PRIMARY KEY,
  ruleset_version text NOT NULL,
  ruleset_hash char(64) NOT NULL UNIQUE,
  body_json jsonb NOT NULL
);

CREATE TABLE sessions (
  session_id text PRIMARY KEY,
  principal_id text NOT NULL,
  status text NOT NULL,
  version numeric(20,0) NOT NULL DEFAULT 0 CHECK (version >= 0),
  ruleset_id text REFERENCES rulesets(ruleset_id),
  commitment_hash char(64),
  created_at_ns bigint NOT NULL
);

CREATE TABLE commands (
  command_id text PRIMARY KEY,
  session_id text NOT NULL REFERENCES sessions(session_id),
  idempotency_key text NOT NULL,
  payload_hash char(64) NOT NULL,
  expected_session_version numeric(20,0) NOT NULL CHECK (expected_session_version >= 0),
  accepted_at_ns bigint NOT NULL,
  payload_json jsonb NOT NULL,
  CONSTRAINT uq_commands_session_idempotency UNIQUE (session_id, idempotency_key)
);

CREATE TABLE domain_events (
  session_id text NOT NULL REFERENCES sessions(session_id),
  event_seq numeric(20,0) NOT NULL CHECK (event_seq >= 0),
  logical_ts_ns bigint NOT NULL,
  event_type text NOT NULL,
  causation_id text NOT NULL,
  correlation_id text NOT NULL,
  payload_json jsonb NOT NULL,
  prior_event_hash char(64) NOT NULL,
  current_event_hash char(64) NOT NULL,
  PRIMARY KEY (session_id, event_seq),
  CONSTRAINT uq_events_session_hash UNIQUE (session_id, current_event_hash)
);

CREATE TABLE snapshots (
  session_id text NOT NULL REFERENCES sessions(session_id),
  event_seq numeric(20,0) NOT NULL CHECK (event_seq >= 0),
  state_version integer NOT NULL CHECK (state_version > 0),
  state_hash char(64) NOT NULL,
  state_json jsonb NOT NULL,
  PRIMARY KEY (session_id, event_seq)
);

CREATE TABLE commitments (
  commitment_id text PRIMARY KEY,
  session_id text NOT NULL REFERENCES sessions(session_id),
  kind text NOT NULL,
  commitment_hash char(64) NOT NULL,
  revealed_secret text,
  CONSTRAINT uq_commitments_session_kind UNIQUE (session_id, kind)
);

CREATE INDEX ix_events_session_causation ON domain_events(session_id, causation_id, event_seq);
COMMIT;
