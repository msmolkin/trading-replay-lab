//! Generated domain-facing v1 models.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentDefinition {
    pub schema_version: String,
    pub instrument_id: String,
    pub venue_id: String,
    pub provider_symbols: BTreeMap<String, String>,
    pub asset_class: String,
    pub product_type: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub settlement_currency: String,
    pub tick_size_atoms: u64,
    pub qty_increment_atoms: u64,
    pub price_scale: u8,
    pub qty_scale: u8,
    pub contract_multiplier_atoms: u64,
    pub multiplier_scale: u8,
    pub settlement_kind: String,
    pub session_calendar: String,
    pub listing_ns: Option<i64>,
    pub expiry_ns: Option<i64>,
    pub effective_from_ns: i64,
    pub effective_through_ns: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderCommand {
    pub command_type: String,
    pub command_id: String,
    pub session_id: String,
    pub instrument_id: String,
    pub side: String,
    pub quantity_atoms: u64,
    pub order_type: String,
    pub limit_price_atoms: Option<i64>,
    pub stop_price_atoms: Option<i64>,
    pub time_in_force: String,
    pub reduce_only: bool,
    pub post_only: bool,
    pub marketable_only: bool,
    pub slippage_cap_bps: Option<u16>,
    pub submitted_at_event_seq: u64,
    pub client_idempotency_key: String,
    pub target_position_atoms: Option<i64>,
    pub quote_event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketEvent {
    pub schema_version: String,
    pub dataset_id: String,
    pub instrument_id: String,
    pub venue_id: String,
    pub ts_event_ns: i64,
    pub ts_recv_ns: Option<i64>,
    pub source_sequence: Option<u64>,
    pub canonical_tie_breaker: u64,
    pub source_event_id: Option<String>,
    pub kind: String,
    pub payload: BTreeMap<String, Value>,
    pub quality_flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionVisibility {
    pub schema_version: String,
    pub phase: String,
    pub revealed_through_ns: i64,
    pub permitted_intervals: Vec<String>,
    pub identity_visibility: String,
    pub calendar_visibility: String,
    pub order_flow_visibility: String,
    pub generation: u64,
}
