"""SQLAlchemy Core schema for the append-only session event store."""

from __future__ import annotations

from sqlalchemy import (
    JSON,
    BigInteger,
    CheckConstraint,
    Column,
    ForeignKey,
    Integer,
    MetaData,
    Numeric,
    String,
    Table,
    Text,
    UniqueConstraint,
)

metadata = MetaData()
U64 = Numeric(20, 0, asdecimal=False)

rulesets = Table(
    "rulesets",
    metadata,
    Column("ruleset_id", String(160), primary_key=True),
    Column("ruleset_version", String(64), nullable=False),
    Column("ruleset_hash", String(64), nullable=False, unique=True),
    Column("body_json", JSON, nullable=False),
)

sessions = Table(
    "sessions",
    metadata,
    Column("session_id", String(160), primary_key=True),
    Column("principal_id", String(160), nullable=False),
    Column("status", String(32), nullable=False),
    Column("version", U64, nullable=False, default=0),
    Column("ruleset_id", ForeignKey("rulesets.ruleset_id"), nullable=True),
    Column("commitment_hash", String(64), nullable=True),
    Column("created_at_ns", BigInteger, nullable=False),
    CheckConstraint("version >= 0", name="ck_sessions_version_nonnegative"),
)

commands = Table(
    "commands",
    metadata,
    Column("command_id", String(160), primary_key=True),
    Column("session_id", ForeignKey("sessions.session_id"), nullable=False),
    Column("idempotency_key", String(200), nullable=False),
    Column("payload_hash", String(64), nullable=False),
    Column("expected_session_version", U64, nullable=False),
    Column("accepted_at_ns", BigInteger, nullable=False),
    Column("payload_json", JSON, nullable=False),
    UniqueConstraint("session_id", "idempotency_key", name="uq_commands_session_idempotency"),
    CheckConstraint("expected_session_version >= 0", name="ck_commands_version_nonnegative"),
)

domain_events = Table(
    "domain_events",
    metadata,
    Column("session_id", ForeignKey("sessions.session_id"), primary_key=True),
    Column("event_seq", U64, primary_key=True),
    Column("logical_ts_ns", BigInteger, nullable=False),
    Column("event_type", String(96), nullable=False),
    Column("causation_id", String(160), nullable=False),
    Column("correlation_id", String(160), nullable=False),
    Column("payload_json", JSON, nullable=False),
    Column("prior_event_hash", String(64), nullable=False),
    Column("current_event_hash", String(64), nullable=False),
    UniqueConstraint("session_id", "current_event_hash", name="uq_events_session_hash"),
    CheckConstraint("event_seq >= 0", name="ck_events_seq_nonnegative"),
)

snapshots = Table(
    "snapshots",
    metadata,
    Column("session_id", ForeignKey("sessions.session_id"), primary_key=True),
    Column("event_seq", U64, primary_key=True),
    Column("state_version", Integer, nullable=False),
    Column("state_hash", String(64), nullable=False),
    Column("state_json", JSON, nullable=False),
    CheckConstraint("event_seq >= 0", name="ck_snapshots_seq_nonnegative"),
    CheckConstraint("state_version > 0", name="ck_snapshots_state_version_positive"),
)

commitments = Table(
    "commitments",
    metadata,
    Column("commitment_id", String(160), primary_key=True),
    Column("session_id", ForeignKey("sessions.session_id"), nullable=False),
    Column("kind", String(64), nullable=False),
    Column("algorithm_version", String(32), nullable=False),
    Column("commitment_hash", String(64), nullable=False),
    Column("setup_hash", String(64), nullable=False),
    Column("eligible_set_hash", String(64), nullable=False),
    Column("metadata_json", JSON, nullable=False),
    Column("sealed_secret", Text, nullable=False),
    Column("revealed_secret", Text, nullable=True),
    UniqueConstraint("session_id", "kind", name="uq_commitments_session_kind"),
)
