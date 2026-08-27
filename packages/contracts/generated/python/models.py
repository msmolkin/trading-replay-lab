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


class SessionVisibility(TypedDict):
    schema_version: Literal["1.0.0"]
    phase: Literal["SETUP", "ACTIVE", "COMPLETED"]
    revealed_through_ns: Int64
    permitted_intervals: list[str]
    identity_visibility: Literal["VISIBLE", "HIDDEN_UNTIL_COMPLETE"]
    calendar_visibility: Literal["ABSOLUTE", "RELATIVE", "HIDDEN_UNTIL_COMPLETE"]
    order_flow_visibility: Literal["NONE", "TRADES", "BBO", "DEPTH", "MBO"]
    generation: UInt64


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
