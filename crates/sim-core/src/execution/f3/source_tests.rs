use crate::numeric::{PriceAtoms, QtyAtoms};
use crate::orders::Side;

use super::{
    F3Error, F3Uncertainty, MboAction, MboBook, MboEvent, MboLevel, MboSnapshot, VisibleOrder,
};

fn source(id: &str, side: Side, price: i64, quantity: u64) -> VisibleOrder {
    VisibleOrder {
        source_order_id: id.into(),
        side,
        price: PriceAtoms::new(price),
        quantity: QtyAtoms::new(quantity),
    }
}

fn snapshot(sequence: u64) -> MboSnapshot {
    MboSnapshot {
        sequence,
        orders: vec![
            source("b1", Side::Buy, 100, 4),
            source("b2", Side::Buy, 100, 3),
            source("b3", Side::Buy, 99, 5),
            source("a1", Side::Sell, 101, 2),
            source("a2", Side::Sell, 101, 4),
            source("a3", Side::Sell, 102, 6),
        ],
    }
}

#[test]
fn snapshot_reconstructs_per_order_priority_and_top_levels() {
    let mut book = MboBook::new();
    let outcome = book.apply_snapshot(snapshot(10)).unwrap();
    assert!(outcome.uncertainty.is_empty());
    assert!(book.is_enabled());
    assert_eq!(book.sequence(), Some(10));
    assert_eq!(
        book.order_ids_at(Side::Buy, PriceAtoms::new(100)),
        vec!["b1", "b2"]
    );
    assert_eq!(
        book.best_bid().unwrap(),
        Some(MboLevel {
            price: PriceAtoms::new(100),
            quantity: QtyAtoms::new(7),
            order_count: 2,
        })
    );
    assert_eq!(
        book.best_ask().unwrap(),
        Some(MboLevel {
            price: PriceAtoms::new(101),
            quantity: QtyAtoms::new(6),
            order_count: 2,
        })
    );
}

#[test]
fn add_modify_fill_cancel_and_clear_reconstruct_deterministically() {
    let mut book = MboBook::new();
    book.apply_snapshot(snapshot(10)).unwrap();
    book.apply_event(MboEvent {
        sequence: 11,
        action: MboAction::Add(source("b4", Side::Buy, 100, 2)),
    })
    .unwrap();
    assert_eq!(
        book.order_ids_at(Side::Buy, PriceAtoms::new(100)),
        vec!["b1", "b2", "b4"]
    );

    let modified = book
        .apply_event(MboEvent {
            sequence: 12,
            action: MboAction::Modify(source("b1", Side::Buy, 100, 6)),
        })
        .unwrap();
    assert_eq!(
        modified.uncertainty,
        vec![F3Uncertainty::SamePriceModifyPriorityAssumed]
    );
    assert_eq!(
        book.order_ids_at(Side::Buy, PriceAtoms::new(100))[0],
        "b1"
    );

    book.apply_event(MboEvent {
        sequence: 13,
        action: MboAction::Fill {
            source_order_id: "b1".into(),
            side: Side::Buy,
            quantity: QtyAtoms::new(2),
        },
    })
    .unwrap();
    assert_eq!(
        book.visible_order("b1").unwrap().quantity,
        QtyAtoms::new(4)
    );

    book.apply_event(MboEvent {
        sequence: 14,
        action: MboAction::Modify(source("b1", Side::Buy, 98, 4)),
    })
    .unwrap();
    assert_eq!(
        book.order_ids_at(Side::Buy, PriceAtoms::new(100)),
        vec!["b2", "b4"]
    );
    assert_eq!(
        book.order_ids_at(Side::Buy, PriceAtoms::new(98)),
        vec!["b1"]
    );

    book.apply_event(MboEvent {
        sequence: 15,
        action: MboAction::Cancel {
            source_order_id: "b2".into(),
            side: Side::Buy,
        },
    })
    .unwrap();
    assert_eq!(
        book.order_ids_at(Side::Buy, PriceAtoms::new(100)),
        vec!["b4"]
    );

    book.apply_event(MboEvent {
        sequence: 16,
        action: MboAction::Clear { side: Side::Sell },
    })
    .unwrap();
    assert_eq!(book.best_ask().unwrap(), None);
    assert_eq!(book.total_quantity(Side::Sell).unwrap(), 0);
}

#[test]
fn sequence_gap_quarantines_until_reconnect_snapshot() {
    let mut book = MboBook::new();
    book.apply_snapshot(snapshot(10)).unwrap();
    assert_eq!(
        book.apply_event(MboEvent {
            sequence: 12,
            action: MboAction::Cancel {
                source_order_id: "b1".into(),
                side: Side::Buy,
            },
        }),
        Err(F3Error::SequenceGap)
    );
    assert!(!book.is_enabled());
    assert_eq!(book.best_bid().unwrap(), None);
    assert_eq!(
        book.apply_event(MboEvent {
            sequence: 11,
            action: MboAction::Clear { side: Side::Buy },
        }),
        Err(F3Error::BookDisabled)
    );

    let recovered = book.apply_snapshot(snapshot(20)).unwrap();
    assert!(book.is_enabled());
    assert_eq!(book.sequence(), Some(20));
    assert_eq!(
        recovered.uncertainty,
        vec![F3Uncertainty::ReconnectPriorityRebuilt]
    );
}

#[test]
fn malformed_source_actions_quarantine_without_partial_application() {
    let mut book = MboBook::new();
    book.apply_snapshot(snapshot(1)).unwrap();
    assert_eq!(
        book.apply_event(MboEvent {
            sequence: 2,
            action: MboAction::Add(source("b1", Side::Buy, 98, 9)),
        }),
        Err(F3Error::DuplicateSourceOrder)
    );
    assert!(!book.is_enabled());

    book.apply_snapshot(snapshot(10)).unwrap();
    assert_eq!(
        book.apply_event(MboEvent {
            sequence: 11,
            action: MboAction::Fill {
                source_order_id: "a1".into(),
                side: Side::Buy,
                quantity: QtyAtoms::new(1),
            },
        }),
        Err(F3Error::SideMismatch)
    );
    assert!(!book.is_enabled());

    book.apply_snapshot(snapshot(20)).unwrap();
    assert_eq!(
        book.apply_event(MboEvent {
            sequence: 21,
            action: MboAction::Fill {
                source_order_id: "a1".into(),
                side: Side::Sell,
                quantity: QtyAtoms::new(3),
            },
        }),
        Err(F3Error::SourceOverfill)
    );
    assert!(!book.is_enabled());
}
