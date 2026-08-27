//! Generated domain-facing v1 models.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type JsonObject = BTreeMap<String, Value>;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command_type")]
pub enum CommandPayload {
    #[serde(rename = "SUBMIT_ORDER")]
    SubmitOrder {
        instrument_id: String,
        side: String,
        quantity_atoms: u64,
        order_type: String,
        limit_price_atoms: Option<i64>,
        stop_price_atoms: Option<i64>,
        price_reference: Option<String>,
        quote_event_id: Option<String>,
        time_in_force: String,
        reduce_only: bool,
        post_only: bool,
        marketable_only: bool,
    },
    #[serde(rename = "SET_LEVERAGE")]
    SetLeverage { leverage: u8 },
    #[serde(rename = "CANCEL_ORDER")]
    CancelOrder { order_id: String },
    #[serde(rename = "REPLACE_ORDER")]
    ReplaceOrder {
        order_id: String,
        quantity_atoms: Option<u64>,
        limit_price_atoms: Option<i64>,
        stop_price_atoms: Option<i64>,
        time_in_force: Option<String>,
        reduce_only: Option<bool>,
        post_only: Option<bool>,
        marketable_only: Option<bool>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEnvelope {
    pub schema_version: String,
    pub command_id: String,
    pub idempotency_key: String,
    pub session_id: String,
    pub principal_id: String,
    pub accepted_at_ns: i64,
    pub logical_ts_ns: i64,
    pub arrival_seq: u64,
    pub expected_session_version: u64,
    pub payload: CommandPayload,
    pub payload_hash: String,
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
    pub payload: JsonObject,
    pub quality_flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    pub start_ns: i64,
    pub end_ns: i64,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataCapabilities {
    pub bar_intervals: Vec<String>,
    pub has_trades: bool,
    pub has_bbo: bool,
    pub has_l2_snapshots: bool,
    pub has_l2_deltas: bool,
    pub has_l3: bool,
    pub has_mark_price: bool,
    pub has_index_price: bool,
    pub has_funding: bool,
    pub has_open_interest: bool,
    pub has_liquidations: bool,
    pub source_start_ns: i64,
    pub source_end_ns: i64,
    pub known_gaps: Vec<Gap>,
    pub timestamp_resolution_ns: u64,
    pub sequence_quality: String,
    pub redistribution_class: String,
    pub execution_tier: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetManifest {
    pub schema_version: String,
    pub manifest_id: String,
    pub provider: String,
    pub dataset: String,
    pub adapter_version: String,
    pub canonical_schema_version: String,
    pub venue_id: String,
    pub instrument_id: String,
    pub instrument_definition_hash: String,
    pub requested_start_ns: i64,
    pub requested_end_ns: i64,
    pub actual_start_ns: i64,
    pub actual_end_ns: i64,
    pub source_objects: Vec<String>,
    pub source_content_hashes: Vec<String>,
    pub canonical_content_hash: String,
    pub row_counts: BTreeMap<String, u64>,
    pub min_event_ts_ns: i64,
    pub max_event_ts_ns: i64,
    pub min_source_sequence: Option<u64>,
    pub max_source_sequence: Option<u64>,
    pub duplicates_removed: u64,
    pub duplicate_policy: Option<String>,
    pub known_gaps: Vec<Gap>,
    pub crossed_book_intervals: Option<Vec<Gap>>,
    pub stale_quote_intervals: Option<Vec<Gap>>,
    pub quality_decisions: Vec<String>,
    pub redistribution_class: String,
    pub ingested_at_ns: i64,
    pub tool_build_id: String,
    pub status: String,
    pub capabilities: DataCapabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomainEvent {
    pub schema_version: String,
    pub session_id: String,
    pub event_seq: u64,
    pub logical_ts_ns: i64,
    pub event_type: String,
    pub causation_id: String,
    pub correlation_id: String,
    pub payload: JsonObject,
    pub prior_event_hash: String,
    pub current_event_hash: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateHash {
    pub event_seq: u64,
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultMetrics {
    pub survived: bool,
    pub terminal_return_ppb: i64,
    pub max_drawdown_ppb: i64,
    pub peak_effective_leverage_ppb: i64,
    pub benchmark_return_ppb: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultBundle {
    pub schema_version: String,
    pub session_id: String,
    pub setup: JsonObject,
    pub ruleset: JsonObject,
    pub commitments: Vec<String>,
    pub revealed_nonces: Vec<String>,
    pub manifest_hashes: Vec<String>,
    pub commands: Vec<CommandEnvelope>,
    pub domain_events: Vec<DomainEvent>,
    pub state_hashes: Vec<StateHash>,
    pub metrics: ResultMetrics,
    pub result_hash: String,
}
