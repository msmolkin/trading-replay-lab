// @generated domain-facing v1 models. Wire integers are converted to bigint.

export type Int64 = bigint;
export type UInt64 = bigint;
export type Side = "BUY" | "SELL";
export type JsonObject = Readonly<Record<string, unknown>>;

export interface InstrumentDefinition {
  schema_version: "1.0.0";
  instrument_id: string;
  venue_id: string;
  provider_symbols: Readonly<Record<string, string>>;
  asset_class: "CRYPTO" | "EQUITY" | "FUTURE";
  product_type: "SPOT" | "PERPETUAL" | "FUTURE" | "EQUITY";
  base_currency: string;
  quote_currency: string;
  settlement_currency: string;
  tick_size_atoms: UInt64;
  qty_increment_atoms: UInt64;
  price_scale: number;
  qty_scale: number;
  contract_multiplier_atoms: UInt64;
  multiplier_scale: number;
  settlement_kind: "LINEAR" | "INVERSE";
  session_calendar: string;
  listing_ns?: Int64;
  expiry_ns?: Int64;
  effective_from_ns: Int64;
  effective_through_ns?: Int64;
}

export interface OrderCommand {
  command_type: "ORDER";
  command_id: string;
  session_id: string;
  instrument_id: string;
  side: Side;
  quantity_atoms: UInt64;
  order_type: "MARKET" | "LIMIT" | "STOP_MARKET" | "STOP_LIMIT";
  limit_price_atoms?: Int64;
  stop_price_atoms?: Int64;
  time_in_force: "GTC" | "IOC" | "FOK";
  reduce_only: boolean;
  post_only: boolean;
  marketable_only: boolean;
  slippage_cap_bps?: number;
  submitted_at_event_seq: UInt64;
  client_idempotency_key: string;
  target_position_atoms?: Int64;
  quote_event_id?: string;
}

export interface MarketEvent {
  schema_version: "1.0.0";
  dataset_id: string;
  instrument_id: string;
  venue_id: string;
  ts_event_ns: Int64;
  ts_recv_ns?: Int64;
  source_sequence?: UInt64;
  canonical_tie_breaker: UInt64;
  source_event_id?: string;
  kind: string;
  payload: JsonObject;
  quality_flags: readonly string[];
}

export interface DataCapabilities {
  bar_intervals: readonly string[];
  has_trades: boolean;
  has_bbo: boolean;
  has_l2_snapshots: boolean;
  has_l2_deltas: boolean;
  has_l3: boolean;
  has_mark_price: boolean;
  has_index_price: boolean;
  has_funding: boolean;
  has_open_interest: boolean;
  has_liquidations: boolean;
  source_start_ns: Int64;
  source_end_ns: Int64;
  timestamp_resolution_ns: UInt64;
  sequence_quality: "NONE" | "WEAK" | "CONTIGUOUS" | "VENUE_AUTHORITATIVE";
  redistribution_class: "REDISTRIBUTABLE" | "USER_LICENSED" | "RESTRICTED" | "UNKNOWN";
  execution_tier: "F0" | "F0T" | "F1" | "F2" | "F3";
  known_gaps: readonly JsonObject[];
}

export interface DatasetManifest {
  schema_version: "1.0.0";
  manifest_id: string;
  provider: string;
  dataset: string;
  adapter_version: string;
  canonical_schema_version: "1.0.0";
  venue_id: string;
  instrument_id: string;
  instrument_definition_hash: string;
  requested_start_ns: Int64;
  requested_end_ns: Int64;
  actual_start_ns: Int64;
  actual_end_ns: Int64;
  source_objects: readonly string[];
  source_content_hashes: readonly string[];
  canonical_content_hash: string;
  row_counts: Readonly<Record<string, UInt64>>;
  min_event_ts_ns: Int64;
  max_event_ts_ns: Int64;
  duplicates_removed: UInt64;
  known_gaps: readonly JsonObject[];
  quality_decisions: readonly string[];
  redistribution_class: string;
  ingested_at_ns: Int64;
  tool_build_id: string;
  status: "PENDING" | "VALID" | "DEGRADED" | "QUARANTINED" | "REVOKED";
  capabilities: DataCapabilities;
}

export interface SetLeverageCommand {
  command_type: "SET_LEVERAGE";
  requested_leverage: number;
}
export interface CancelOrderCommand {
  command_type: "CANCEL_ORDER";
  order_id: string;
}
export interface ReplaceOrderCommand {
  command_type: "REPLACE_ORDER";
  order_id: string;
  replacement: OrderCommand;
}
export type CommandPayload = OrderCommand | SetLeverageCommand | CancelOrderCommand | ReplaceOrderCommand;

export interface CommandEnvelope {
  schema_version: "1.0.0";
  command_id: string;
  idempotency_key: string;
  session_id: string;
  principal_id: string;
  accepted_at_ns: Int64;
  logical_ts_ns: Int64;
  arrival_seq: UInt64;
  expected_session_version: UInt64;
  payload: CommandPayload;
  payload_hash: string;
}

export interface DomainEvent {
  schema_version: "1.0.0";
  session_id: string;
  event_seq: UInt64;
  logical_ts_ns: Int64;
  event_type: string;
  causation_id: string;
  correlation_id: string;
  payload: JsonObject;
  prior_event_hash: string;
  current_event_hash: string;
}

export interface SessionVisibility {
  schema_version: "1.0.0";
  phase: "SETUP" | "ACTIVE" | "COMPLETED";
  revealed_through_ns: Int64;
  permitted_intervals: readonly string[];
  identity_visibility: "VISIBLE" | "HIDDEN_UNTIL_COMPLETE";
  calendar_visibility: "ABSOLUTE" | "RELATIVE" | "HIDDEN_UNTIL_COMPLETE";
  order_flow_visibility: "NONE" | "TRADES" | "BBO" | "DEPTH" | "MBO";
  generation: UInt64;
}

export interface ResultMetrics {
  survived: boolean;
  terminal_return_ppb: Int64;
  max_drawdown_ppb: Int64;
  peak_effective_leverage_ppb: Int64;
  benchmark_return_ppb: Int64;
}
export interface ResultBundle {
  schema_version: "1.0.0";
  session_id: string;
  setup: JsonObject;
  ruleset: JsonObject;
  commitments: readonly string[];
  revealed_nonces: readonly string[];
  manifest_hashes: readonly string[];
  commands: readonly CommandEnvelope[];
  domain_events: readonly DomainEvent[];
  state_hashes: readonly Readonly<{ event_seq: UInt64; hash: string }>[];
  metrics: ResultMetrics;
  result_hash: string;
}

export function parseInt64(value: string): bigint {
  const parsed = BigInt(value);
  if (parsed < -(1n << 63n) || parsed > (1n << 63n) - 1n) throw new RangeError("int64");
  return parsed;
}
export function parseUInt64(value: string): bigint {
  const parsed = BigInt(value);
  if (parsed < 0n || parsed > (1n << 64n) - 1n) throw new RangeError("uint64");
  return parsed;
}
export function toWireInteger(value: bigint): string {
  return value.toString(10);
}
