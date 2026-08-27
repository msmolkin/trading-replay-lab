"""Generated domain-facing v1 models. Wire integers become Python ints."""

from __future__ import annotations

from typing import Literal, NotRequired, TypedDict

Int64 = int
UInt64 = int
JsonObject = dict[str, object]
OrderType = Literal["MARKET", "LIMIT", "STOP_MARKET", "STOP_LIMIT"]
TimeInForce = Literal["GTC", "IOC", "FOK"]
PriceReference = Literal["BID", "ASK", "MIDPOINT"]


class InstrumentDefinition(TypedDict):
    schema_version: Literal["1.0.0"]
    instrument_id: str
    venue_id: str
    provider_symbols: dict[str, str]
    asset_class: Literal["CRYPTO", "EQUITY", "FUTURE"]
    product_type: Literal["SPOT", "PERPETUAL", "FUTURE", "EQUITY"]
    base_currency: str
    quote_currency: str
    settlement_currency: str
    tick_size_atoms: UInt64
    qty_increment_atoms: UInt64
    price_scale: int
    qty_scale: int
    contract_multiplier_atoms: UInt64
    multiplier_scale: int
    settlement_kind: Literal["LINEAR", "INVERSE"]
    session_calendar: str
    listing_ns: NotRequired[Int64]
    expiry_ns: NotRequired[Int64]
    effective_from_ns: Int64
    effective_through_ns: NotRequired[Int64]


class OrderCommand(TypedDict):
    """Legacy full order wire contract from order.schema.json."""

    command_type: Literal["ORDER"]
    command_id: str
    session_id: str
    instrument_id: str
    side: Literal["BUY", "SELL"]
    quantity_atoms: UInt64
    order_type: OrderType
    limit_price_atoms: NotRequired[Int64]
    stop_price_atoms: NotRequired[Int64]
    time_in_force: TimeInForce
    reduce_only: bool
    post_only: bool
    marketable_only: bool
    slippage_cap_bps: NotRequired[int]
    submitted_at_event_seq: UInt64
    client_idempotency_key: str
    target_position_atoms: NotRequired[Int64]
    quote_event_id: NotRequired[str]


class SubmitOrderCommand(TypedDict):
    command_type: Literal["SUBMIT_ORDER"]
    instrument_id: str
    side: Literal["BUY", "SELL"]
    quantity_atoms: UInt64
    order_type: OrderType
    limit_price_atoms: NotRequired[Int64]
    stop_price_atoms: NotRequired[Int64]
    price_reference: NotRequired[PriceReference]
    quote_event_id: NotRequired[str]
    time_in_force: TimeInForce
    reduce_only: bool
    post_only: bool
    marketable_only: bool


class SetLeverageCommand(TypedDict):
    command_type: Literal["SET_LEVERAGE"]
    leverage: int


class CancelOrderCommand(TypedDict):
    command_type: Literal["CANCEL_ORDER"]
    order_id: str


class ReplaceOrderCommand(TypedDict):
    command_type: Literal["REPLACE_ORDER"]
    order_id: str
    quantity_atoms: NotRequired[UInt64]
    limit_price_atoms: NotRequired[Int64]
    stop_price_atoms: NotRequired[Int64]
    time_in_force: NotRequired[TimeInForce]
    reduce_only: NotRequired[bool]
    post_only: NotRequired[bool]
    marketable_only: NotRequired[bool]


CommandPayload = SubmitOrderCommand | SetLeverageCommand | CancelOrderCommand | ReplaceOrderCommand


class CommandEnvelope(TypedDict):
    schema_version: Literal["1.0.0"]
    command_id: str
    idempotency_key: str
    session_id: str
    principal_id: str
    accepted_at_ns: Int64
    logical_ts_ns: Int64
    arrival_seq: UInt64
    expected_session_version: UInt64
    payload: CommandPayload
    payload_hash: str


class MarketEvent(TypedDict):
    schema_version: Literal["1.0.0"]
    dataset_id: str
    instrument_id: str
    venue_id: str
    ts_event_ns: Int64
    ts_recv_ns: NotRequired[Int64]
    source_sequence: NotRequired[UInt64]
    canonical_tie_breaker: UInt64
    source_event_id: NotRequired[str]
    kind: str
    payload: JsonObject
    quality_flags: list[str]


class Gap(TypedDict):
    start_ns: Int64
    end_ns: Int64
    reason: str


class DataCapabilities(TypedDict):
    bar_intervals: list[str]
    has_trades: bool
    has_bbo: bool
    has_l2_snapshots: bool
    has_l2_deltas: bool
    has_l3: bool
    has_mark_price: bool
    has_index_price: bool
    has_funding: bool
    has_open_interest: bool
    has_liquidations: bool
    source_start_ns: Int64
    source_end_ns: Int64
    known_gaps: list[Gap]
    timestamp_resolution_ns: UInt64
    sequence_quality: Literal["NONE", "WEAK", "CONTIGUOUS", "VENUE_AUTHORITATIVE"]
    redistribution_class: Literal["REDISTRIBUTABLE", "USER_LICENSED", "RESTRICTED", "UNKNOWN"]
    execution_tier: Literal["F0", "F0T", "F1", "F2", "F3"]


class DatasetManifest(TypedDict):
    schema_version: Literal["1.0.0"]
    manifest_id: str
    provider: str
    dataset: str
    adapter_version: str
    canonical_schema_version: Literal["1.0.0"]
    venue_id: str
    instrument_id: str
    instrument_definition_hash: str
    requested_start_ns: Int64
    requested_end_ns: Int64
    actual_start_ns: Int64
    actual_end_ns: Int64
    source_objects: list[str]
    source_content_hashes: list[str]
    canonical_content_hash: str
    row_counts: dict[str, UInt64]
    min_event_ts_ns: Int64
    max_event_ts_ns: Int64
    min_source_sequence: NotRequired[UInt64]
    max_source_sequence: NotRequired[UInt64]
    duplicates_removed: UInt64
    duplicate_policy: NotRequired[str]
    known_gaps: list[Gap]
    crossed_book_intervals: NotRequired[list[Gap]]
    stale_quote_intervals: NotRequired[list[Gap]]
    quality_decisions: list[str]
    redistribution_class: Literal["REDISTRIBUTABLE", "USER_LICENSED", "RESTRICTED", "UNKNOWN"]
    ingested_at_ns: Int64
    tool_build_id: str
    status: Literal["PENDING", "VALID", "DEGRADED", "QUARANTINED", "REVOKED"]
    capabilities: DataCapabilities


class DomainEvent(TypedDict):
    schema_version: Literal["1.0.0"]
    session_id: str
    event_seq: UInt64
    logical_ts_ns: Int64
    event_type: str
    causation_id: str
    correlation_id: str
    payload: JsonObject
    prior_event_hash: str
    current_event_hash: str


class SessionVisibility(TypedDict):
    schema_version: Literal["1.0.0"]
    phase: Literal["SETUP", "ACTIVE", "COMPLETED"]
    revealed_through_ns: Int64
    permitted_intervals: list[str]
    identity_visibility: Literal["VISIBLE", "HIDDEN_UNTIL_COMPLETE"]
    calendar_visibility: Literal["ABSOLUTE", "RELATIVE", "HIDDEN_UNTIL_COMPLETE"]
    order_flow_visibility: Literal["NONE", "TRADES", "BBO", "DEPTH", "MBO"]
    generation: UInt64


class StateHash(TypedDict):
    event_seq: UInt64
    hash: str


class ResultMetrics(TypedDict):
    survived: bool
    terminal_return_ppb: Int64
    max_drawdown_ppb: Int64
    peak_effective_leverage_ppb: Int64
    benchmark_return_ppb: Int64


class ResultBundle(TypedDict):
    schema_version: Literal["1.0.0"]
    session_id: str
    setup: JsonObject
    ruleset: JsonObject
    commitments: list[str]
    revealed_nonces: list[str]
    manifest_hashes: list[str]
    commands: list[CommandEnvelope]
    domain_events: list[DomainEvent]
    state_hashes: list[StateHash]
    metrics: ResultMetrics
    result_hash: str


def parse_int64(value: str) -> int:
    parsed = int(value, 10)
    if parsed < -(2**63) or parsed > 2**63 - 1:
        raise ValueError("int64")
    return parsed


def parse_uint64(value: str) -> int:
    parsed = int(value, 10)
    if parsed < 0 or parsed > 2**64 - 1:
        raise ValueError("uint64")
    return parsed


def to_wire_integer(value: int) -> str:
    return str(value)
