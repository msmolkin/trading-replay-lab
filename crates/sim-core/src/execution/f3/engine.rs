use std::collections::BTreeSet;

use crate::numeric::{PriceAtoms, QtyAtoms};
use crate::orders::{OrderError, OrderId, OrderKind, OrderState, Side, TimeInForce};
#[cfg(test)]
use crate::orders::OrderStatus;

use super::book::MboBook;
use super::types::{
    CounterfactualPlayer, F3Error, F3EventOutcome, F3Uncertainty, MboAction, MboApplyOutcome,
    MboEvent, MboSnapshot, PlayerImpactCaps, PlayerInsertion, VisibleOrder,
};

const PPM_DENOMINATOR: u128 = 1_000_000;

impl CounterfactualPlayer {
    /// Exact displayed source quantity still modeled ahead of the player.
    ///
    /// # Errors
    /// Returns [`F3Error::PlayerInvalidated`] after continuity/clear invalidation, or
    /// [`F3Error::InvalidQueue`] if reconstructed source facts no longer match the queue marker.
    pub fn ahead_quantity(&self, book: &MboBook) -> Result<QtyAtoms, F3Error> {
        if !self.valid || !book.enabled {
            return Err(F3Error::PlayerInvalidated);
        }
        let total = self
            .ahead_source_orders
            .iter()
            .try_fold(0_u64, |total, source_id| {
                let order = book.orders.get(source_id).ok_or(F3Error::InvalidQueue)?;
                if order.side != self.side || order.price != self.price {
                    return Err(F3Error::InvalidQueue);
                }
                total
                    .checked_add(order.quantity.get())
                    .ok_or(F3Error::QuantityArithmetic)
            })?;
        Ok(QtyAtoms::new(total))
    }
}

/// Inserts one already-accepted simulator GTC limit order into the current MBO queue without
/// modifying historical source orders.
///
/// The insertion must occur at exactly the source frontier recorded on order acceptance. The
/// order joins behind every source order then visible at its price. Hard caps bound absolute size,
/// same-side displayed share, BBO improvement, and queue marker cardinality.
///
/// # Errors
/// Returns [`F3Error`] without mutating either authoritative order or source book state.
pub fn insert_player(
    orders: &OrderState,
    book: &MboBook,
    order_id: OrderId,
    caps: PlayerImpactCaps,
) -> Result<PlayerInsertion, F3Error> {
    if !book.enabled {
        return Err(F3Error::BookDisabled);
    }
    if caps.max_order_quantity.get() == 0 || caps.max_side_fraction_ppm > 1_000_000 {
        return Err(F3Error::PlayerImpactCapExceeded);
    }
    let order = orders.get(order_id).ok_or(OrderError::UnknownOrder)?;
    if !order.is_executable() || order.time_in_force != TimeInForce::Gtc {
        return Err(F3Error::UnsupportedPlayerOrder);
    }
    let OrderKind::Limit { limit_price } = order.kind else {
        return Err(F3Error::UnsupportedPlayerOrder);
    };
    let sequence = book.sequence.ok_or(F3Error::BookDisabled)?;
    if order.submitted_at_event_seq != sequence {
        return Err(F3Error::PlayerFrontierMismatch);
    }
    let remaining = order.remaining();
    if remaining.get() == 0 || remaining > caps.max_order_quantity {
        return Err(F3Error::PlayerSizeCapExceeded);
    }

    let side_total = book.total_quantity(order.side)?;
    if side_total == 0 {
        return Err(F3Error::PlayerImpactCapExceeded);
    }
    let scaled_player = u128::from(remaining.get())
        .checked_mul(PPM_DENOMINATOR)
        .ok_or(F3Error::QuantityArithmetic)?;
    let permitted = side_total
        .checked_mul(u128::from(caps.max_side_fraction_ppm))
        .ok_or(F3Error::QuantityArithmetic)?;
    if scaled_player > permitted {
        return Err(F3Error::PlayerImpactCapExceeded);
    }

    let improvement = bbo_improvement(book, order.side, limit_price)?;
    if improvement > caps.max_bbo_improvement_atoms {
        return Err(F3Error::PlayerBboImpactExceeded);
    }
    if is_marketable(book, order.side, limit_price)? {
        return Err(F3Error::UnsupportedPlayerOrder);
    }

    let ahead_ids = book
        .queue(order.side, limit_price.get())
        .cloned()
        .unwrap_or_default();
    if ahead_ids.len() > caps.max_source_orders_ahead {
        return Err(F3Error::PlayerQueueCapExceeded);
    }
    let ahead_source_orders: BTreeSet<String> = ahead_ids.into_iter().collect();
    let player = CounterfactualPlayer {
        order_id,
        side: order.side,
        price: limit_price,
        ahead_source_orders,
        valid: true,
    };
    let ahead_quantity = player.ahead_quantity(book)?;
    let mut uncertainty = vec![F3Uncertainty::CounterfactualPlayerImpact];
    if improvement > 0 {
        uncertainty.push(F3Uncertainty::PlayerBboImprovement);
    }
    Ok(PlayerInsertion {
        player,
        ahead_quantity,
        uncertainty,
    })
}

/// Applies a complete recovery snapshot while safely invalidating any existing counterfactual
/// player queue marker.
///
/// A source snapshot cannot prove where the hypothetical player would sit relative to rebuilt
/// source priority, so an existing player must be reinserted explicitly at the new frontier. If a
/// recovery snapshot itself is malformed, the prior source book is quarantined and the player is
/// invalidated rather than continuing on stale queue facts.
///
/// # Errors
/// Returns [`F3Error`] from snapshot validation. Recovery failures quarantine source execution.
pub fn apply_snapshot_with_player(
    book: &mut MboBook,
    player: &mut CounterfactualPlayer,
    snapshot: MboSnapshot,
) -> Result<MboApplyOutcome, F3Error> {
    let recovering = book.sequence.is_some();
    match book.apply_snapshot(snapshot) {
        Ok(outcome) => {
            if recovering {
                player.invalidate();
            }
            Ok(outcome)
        }
        Err(error) => {
            if recovering {
                book.enabled = false;
                player.invalidate();
            }
            Err(error)
        }
    }
}

/// Applies one source MBO event and atomically updates an inserted player's queue/fill state.
///
/// A valid source event is authoritative even if the counterfactual player overlay later fails.
/// In that case the source frontier still advances, the player marker is invalidated, and the
/// simulator order state remains unchanged. Source continuity/structural failures similarly commit
/// the quarantined source state and invalidate the player.
///
/// # Errors
/// Returns [`F3Error`] for source reconstruction, invalidated player state, or simulator order
/// transition failures.
pub fn apply_event_with_player(
    orders: &mut OrderState,
    book: &mut MboBook,
    player: &mut CounterfactualPlayer,
    event: MboEvent,
) -> Result<F3EventOutcome, F3Error> {
    if !player.valid {
        return Err(F3Error::PlayerInvalidated);
    }
    let before = source_order_for_action(book, &event.action).cloned();
    let action = event.action.clone();
    let mut next_book = book.clone();
    let apply = match next_book.apply_event(event) {
        Ok(outcome) => outcome,
        Err(error) => {
            *book = next_book;
            player.invalidate();
            return Err(error);
        }
    };

    let mut next_orders = orders.clone();
    let mut next_player = player.clone();
    let mut uncertainty = apply.uncertainty;
    let player_fill = match update_player_for_action(
        &mut next_orders,
        &next_book,
        &mut next_player,
        &action,
        before.as_ref(),
        &mut uncertainty,
    ) {
        Ok(fill) => fill,
        Err(error) => {
            *book = next_book;
            player.invalidate();
            return Err(error);
        }
    };
    let Some(player_order) = next_orders.get(next_player.order_id) else {
        *book = next_book;
        player.invalidate();
        return Err(OrderError::UnknownOrder.into());
    };
    let player_status = player_order.status;
    *orders = next_orders;
    *book = next_book;
    *player = next_player;
    Ok(F3EventOutcome {
        sequence: apply.sequence,
        player_filled: player_fill,
        player_status,
        uncertainty,
    })
}

fn source_order_for_action<'a>(book: &'a MboBook, action: &MboAction) -> Option<&'a VisibleOrder> {
    let source_id = match action {
        MboAction::Add(_) | MboAction::Clear { .. } => return None,
        MboAction::Modify(order) => order.source_order_id.as_str(),
        MboAction::Cancel {
            source_order_id, ..
        }
        | MboAction::Fill {
            source_order_id, ..
        } => source_order_id,
    };
    book.orders.get(source_id)
}

fn update_player_for_action(
    orders: &mut OrderState,
    book: &MboBook,
    player: &mut CounterfactualPlayer,
    action: &MboAction,
    before: Option<&VisibleOrder>,
    uncertainty: &mut Vec<F3Uncertainty>,
) -> Result<QtyAtoms, F3Error> {
    if let MboAction::Clear { side } = action {
        if *side == player.side {
            player.invalidate();
            uncertainty.push(F3Uncertainty::ClearInvalidatedPlayer);
        }
        return Ok(QtyAtoms::new(0));
    }

    let Some(before) = before else {
        return Ok(QtyAtoms::new(0));
    };
    let was_ahead = player.ahead_source_orders.contains(&before.source_order_id);
    let after = book.orders.get(&before.source_order_id);
    if was_ahead {
        let still_ahead =
            after.is_some_and(|order| order.side == player.side && order.price == player.price);
        if !still_ahead {
            player.ahead_source_orders.remove(&before.source_order_id);
        }
        return Ok(QtyAtoms::new(0));
    }

    let MboAction::Fill { quantity, .. } = action else {
        return Ok(QtyAtoms::new(0));
    };
    if before.side != player.side || before.price != player.price {
        return Ok(QtyAtoms::new(0));
    }
    if player.ahead_quantity(book)?.get() != 0 {
        return Ok(QtyAtoms::new(0));
    }
    let player_order = orders
        .get(player.order_id)
        .ok_or(OrderError::UnknownOrder)?
        .clone();
    if !player_order.is_executable() {
        return Ok(QtyAtoms::new(0));
    }
    let fill = QtyAtoms::new(quantity.get().min(player_order.remaining().get()));
    if fill.get() == 0 {
        return Ok(fill);
    }
    orders.record_fill(player.order_id, fill)?;
    uncertainty.push(F3Uncertainty::HistoricalFillWouldCrossPlayer);
    Ok(fill)
}

fn bbo_improvement(book: &MboBook, side: Side, price: PriceAtoms) -> Result<u64, F3Error> {
    match side {
        Side::Buy => {
            let best = book.best_bid()?.ok_or(F3Error::PlayerImpactCapExceeded)?;
            if price <= best.price {
                return Ok(0);
            }
            u64::try_from(price.get() - best.price.get()).map_err(|_| F3Error::QuantityArithmetic)
        }
        Side::Sell => {
            let best = book.best_ask()?.ok_or(F3Error::PlayerImpactCapExceeded)?;
            if price >= best.price {
                return Ok(0);
            }
            u64::try_from(best.price.get() - price.get()).map_err(|_| F3Error::QuantityArithmetic)
        }
    }
}

fn is_marketable(book: &MboBook, side: Side, price: PriceAtoms) -> Result<bool, F3Error> {
    match side {
        Side::Buy => Ok(book.best_ask()?.is_some_and(|ask| price >= ask.price)),
        Side::Sell => Ok(book.best_bid()?.is_some_and(|bid| price <= bid.price)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orders::NewOrder;

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

    fn wide_snapshot(sequence: u64) -> MboSnapshot {
        MboSnapshot {
            sequence,
            orders: vec![
                source("b1", Side::Buy, 100, 10),
                source("a1", Side::Sell, 103, 10),
            ],
        }
    }

    fn submit_player(
        orders: &mut OrderState,
        side: Side,
        price: i64,
        quantity: u64,
        submitted_at_event_seq: u64,
    ) -> OrderId {
        orders
            .submit(
                NewOrder {
                    client_order_id: "player".into(),
                    instrument_id: "SYNTH".into(),
                    side,
                    quantity: QtyAtoms::new(quantity),
                    kind: OrderKind::Limit {
                        limit_price: PriceAtoms::new(price),
                    },
                    time_in_force: TimeInForce::Gtc,
                    reduce_only: false,
                    post_only: false,
                    marketable_only: false,
                    submitted_at_event_seq,
                },
                0,
                None,
            )
            .unwrap()
            .order_id
    }

    fn permissive_caps() -> PlayerImpactCaps {
        PlayerImpactCaps {
            max_order_quantity: QtyAtoms::new(100),
            max_side_fraction_ppm: 1_000_000,
            max_bbo_improvement_atoms: 10,
            max_source_orders_ahead: 100,
        }
    }

    #[test]
    fn player_joins_exact_frontier_behind_visible_queue() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Sell, 101, 2, 10);
        let insertion = insert_player(&orders, &book, id, permissive_caps()).unwrap();
        assert_eq!(insertion.ahead_quantity, QtyAtoms::new(6));
        assert_eq!(insertion.player.ahead_order_count(), 2);
        assert_eq!(
            insertion.uncertainty,
            vec![F3Uncertainty::CounterfactualPlayerImpact]
        );
    }

    #[test]
    fn stale_or_future_player_frontier_is_rejected() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let stale = submit_player(&mut orders, Side::Buy, 100, 1, 9);
        let future = submit_player(&mut orders, Side::Buy, 100, 1, 11);
        assert_eq!(
            insert_player(&orders, &book, stale, permissive_caps()),
            Err(F3Error::PlayerFrontierMismatch)
        );
        assert_eq!(
            insert_player(&orders, &book, future, permissive_caps()),
            Err(F3Error::PlayerFrontierMismatch)
        );
    }

    #[test]
    fn size_share_bbo_and_queue_caps_fail_closed() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let size_id = submit_player(&mut orders, Side::Buy, 100, 8, 10);
        assert_eq!(
            insert_player(
                &orders,
                &book,
                size_id,
                PlayerImpactCaps {
                    max_order_quantity: QtyAtoms::new(7),
                    ..permissive_caps()
                },
            ),
            Err(F3Error::PlayerSizeCapExceeded)
        );
        assert_eq!(
            insert_player(
                &orders,
                &book,
                size_id,
                PlayerImpactCaps {
                    max_side_fraction_ppm: 100_000,
                    ..permissive_caps()
                },
            ),
            Err(F3Error::PlayerImpactCapExceeded)
        );

        let improve_id = submit_player(&mut orders, Side::Buy, 101, 1, 10);
        assert_eq!(
            insert_player(
                &orders,
                &book,
                improve_id,
                PlayerImpactCaps {
                    max_bbo_improvement_atoms: 0,
                    ..permissive_caps()
                },
            ),
            Err(F3Error::PlayerBboImpactExceeded)
        );

        let queue_id = submit_player(&mut orders, Side::Sell, 101, 1, 10);
        assert_eq!(
            insert_player(
                &orders,
                &book,
                queue_id,
                PlayerImpactCaps {
                    max_source_orders_ahead: 1,
                    ..permissive_caps()
                },
            ),
            Err(F3Error::PlayerQueueCapExceeded)
        );
    }

    #[test]
    fn permitted_bbo_improvement_is_explicitly_uncertain() {
        let mut book = MboBook::new();
        book.apply_snapshot(wide_snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Buy, 101, 1, 10);
        let insertion = insert_player(&orders, &book, id, permissive_caps()).unwrap();
        assert_eq!(insertion.ahead_quantity, QtyAtoms::new(0));
        assert!(
            insertion
                .uncertainty
                .contains(&F3Uncertainty::CounterfactualPlayerImpact)
        );
        assert!(
            insertion
                .uncertainty
                .contains(&F3Uncertainty::PlayerBboImprovement)
        );
    }

    #[test]
    fn historical_fill_behind_player_fills_player_after_ahead_clears() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Sell, 101, 5, 10);
        let insertion = insert_player(&orders, &book, id, permissive_caps()).unwrap();
        let mut player = insertion.player;

        apply_event_with_player(
            &mut orders,
            &mut book,
            &mut player,
            MboEvent {
                sequence: 11,
                action: MboAction::Add(source("a4", Side::Sell, 101, 3)),
            },
        )
        .unwrap();
        apply_event_with_player(
            &mut orders,
            &mut book,
            &mut player,
            MboEvent {
                sequence: 12,
                action: MboAction::Fill {
                    source_order_id: "a1".into(),
                    side: Side::Sell,
                    quantity: QtyAtoms::new(2),
                },
            },
        )
        .unwrap();
        apply_event_with_player(
            &mut orders,
            &mut book,
            &mut player,
            MboEvent {
                sequence: 13,
                action: MboAction::Cancel {
                    source_order_id: "a2".into(),
                    side: Side::Sell,
                },
            },
        )
        .unwrap();
        assert_eq!(player.ahead_quantity(&book).unwrap(), QtyAtoms::new(0));

        let outcome = apply_event_with_player(
            &mut orders,
            &mut book,
            &mut player,
            MboEvent {
                sequence: 14,
                action: MboAction::Fill {
                    source_order_id: "a4".into(),
                    side: Side::Sell,
                    quantity: QtyAtoms::new(3),
                },
            },
        )
        .unwrap();
        assert_eq!(outcome.player_filled, QtyAtoms::new(3));
        assert_eq!(outcome.player_status, OrderStatus::PartiallyFilled);
        assert!(
            outcome
                .uncertainty
                .contains(&F3Uncertainty::HistoricalFillWouldCrossPlayer)
        );
        assert_eq!(orders.get(id).unwrap().filled, QtyAtoms::new(3));
    }

    #[test]
    fn source_clear_invalidates_counterfactual_player() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Sell, 101, 1, 10);
        let mut player = insert_player(&orders, &book, id, permissive_caps())
            .unwrap()
            .player;
        let outcome = apply_event_with_player(
            &mut orders,
            &mut book,
            &mut player,
            MboEvent {
                sequence: 11,
                action: MboAction::Clear { side: Side::Sell },
            },
        )
        .unwrap();
        assert!(!player.is_valid());
        assert_eq!(outcome.player_filled, QtyAtoms::new(0));
        assert!(
            outcome
                .uncertainty
                .contains(&F3Uncertainty::ClearInvalidatedPlayer)
        );
    }

    #[test]
    fn source_gap_invalidates_player_and_commits_quarantine() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Buy, 100, 1, 10);
        let mut player = insert_player(&orders, &book, id, permissive_caps())
            .unwrap()
            .player;
        assert_eq!(
            apply_event_with_player(
                &mut orders,
                &mut book,
                &mut player,
                MboEvent {
                    sequence: 12,
                    action: MboAction::Clear { side: Side::Sell },
                },
            ),
            Err(F3Error::SequenceGap)
        );
        assert!(!book.is_enabled());
        assert!(!player.is_valid());
        assert_eq!(orders.get(id).unwrap().filled, QtyAtoms::new(0));
    }

    #[test]
    fn reconnect_snapshot_invalidates_player_and_reports_rebuilt_priority() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Buy, 100, 1, 10);
        let mut player = insert_player(&orders, &book, id, permissive_caps())
            .unwrap()
            .player;
        let outcome = apply_snapshot_with_player(&mut book, &mut player, snapshot(20)).unwrap();
        assert!(!player.is_valid());
        assert_eq!(book.sequence(), Some(20));
        assert_eq!(
            outcome.uncertainty,
            vec![F3Uncertainty::ReconnectPriorityRebuilt]
        );
    }

    #[test]
    fn malformed_reconnect_quarantines_book_and_player() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Buy, 100, 1, 10);
        let mut player = insert_player(&orders, &book, id, permissive_caps())
            .unwrap()
            .player;
        let invalid = MboSnapshot {
            sequence: 20,
            orders: vec![source("bad", Side::Buy, 0, 1)],
        };
        assert_eq!(
            apply_snapshot_with_player(&mut book, &mut player, invalid),
            Err(F3Error::InvalidBook)
        );
        assert!(!book.is_enabled());
        assert!(!player.is_valid());
    }

    #[test]
    fn player_overlay_failure_still_commits_authoritative_source_frontier() {
        let mut book = MboBook::new();
        book.apply_snapshot(snapshot(10)).unwrap();
        let mut orders = OrderState::new();
        let id = submit_player(&mut orders, Side::Buy, 100, 1, 10);
        let mut player = insert_player(&orders, &book, id, permissive_caps())
            .unwrap()
            .player;
        orders = OrderState::new();

        assert_eq!(
            apply_event_with_player(
                &mut orders,
                &mut book,
                &mut player,
                MboEvent {
                    sequence: 11,
                    action: MboAction::Add(source("b4", Side::Buy, 98, 2)),
                },
            ),
            Err(F3Error::Order(OrderError::UnknownOrder))
        );
        assert_eq!(book.sequence(), Some(11));
        assert!(book.visible_order("b4").is_some());
        assert!(!player.is_valid());
    }
}
