//! F3 market-by-order reconstruction and counterfactual player queue modeling.

mod book;
mod engine;
mod types;

pub use book::MboBook;
pub use engine::{apply_event_with_player, apply_snapshot_with_player, insert_player};
pub use types::{
    CounterfactualPlayer, F3Error, F3EventOutcome, F3Uncertainty, MboAction, MboApplyOutcome,
    MboEvent, MboLevel, MboSnapshot, PlayerImpactCaps, PlayerInsertion, VisibleOrder,
};
