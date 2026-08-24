//! Versioned authoritative simulator snapshots.

use crate::economics::EconomicsState;
use crate::execution::f2::L2Book;
use crate::facade::FacadeConfig;
use crate::kernel::KernelSnapshot;
use crate::ledger::LedgerSnapshot;
use crate::orders::OrderState;
use crate::positions::Position;
use crate::risk::RiskState;

/// Current in-process simulator snapshot format.
pub const SNAPSHOT_FORMAT_VERSION: u16 = 1;

/// Complete deterministic continuation state for the M1 simulator facade.
///
/// State-machine types with private internals are stored directly rather than projected into
/// parallel public fields. This prevents a restore format from silently dropping order ids,
/// economics idempotency fingerprints, liquidation state, or reconstructed depth continuity.
/// Forgeable public snapshots (kernel, ledger, position) are revalidated by the facade on restore.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorSnapshot {
    /// Snapshot schema/compatibility version.
    pub format_version: u16,
    /// Immutable simulator rules/configuration bound into deterministic continuation.
    pub config: FacadeConfig,
    /// Last accepted logical time; `None` before the first input.
    pub last_logical_ts_ns: Option<i64>,
    /// Last market-data sequence observed by an execution input.
    pub market_event_seq: Option<u64>,
    /// Sequencing/version/hash-chain continuation state.
    pub kernel: KernelSnapshot,
    /// Full order lifecycle state including next id and revisions.
    pub orders: OrderState,
    /// Current economic position.
    pub position: Position,
    /// Current balanced ledger state. Historical transactions live in the domain-event stream.
    pub ledger: LedgerSnapshot,
    /// Scheduled-economics idempotency state.
    pub economics: EconomicsState,
    /// Leverage/liquidation state.
    pub risk: RiskState,
    /// Reconstructed L2 book when the committed execution tier is F2.
    pub f2_book: Option<L2Book>,
}
