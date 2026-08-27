use std::collections::BTreeMap;

use crate::numeric::{PriceAtoms, QtyAtoms};
use crate::orders::Side;

use super::types::{
    F3Error, F3Uncertainty, MboAction, MboApplyOutcome, MboEvent, MboLevel, MboSnapshot,
    VisibleOrder,
};

const MAX_SOURCE_ORDER_ID_BYTES: usize = 4096;

/// Sequence-aware authoritative market-by-order reconstruction.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MboBook {
    pub(super) sequence: Option<u64>,
    pub(super) enabled: bool,
    pub(super) orders: BTreeMap<String, VisibleOrder>,
    bids: BTreeMap<i64, Vec<String>>,
    asks: BTreeMap<i64, Vec<String>>,
}

impl MboBook {
    /// Creates an empty disabled book. A full snapshot is required before use.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sequence: None,
            enabled: false,
            orders: BTreeMap::new(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        }
    }

    /// Whether exact source continuity currently permits execution/queue use.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Current authoritative source sequence.
    #[must_use]
    pub const fn sequence(&self) -> Option<u64> {
        self.sequence
    }

    /// Visible source order by provider id.
    #[must_use]
    pub fn visible_order(&self, source_order_id: &str) -> Option<&VisibleOrder> {
        self.enabled
            .then(|| self.orders.get(source_order_id))
            .flatten()
    }

    /// Source ids at one exact price in front-to-back queue order.
    #[must_use]
    pub fn order_ids_at(&self, side: Side, price: PriceAtoms) -> Vec<&str> {
        if !self.enabled {
            return Vec::new();
        }
        self.queue(side, price.get())
            .map_or_else(Vec::new, |ids| ids.iter().map(String::as_str).collect())
    }

    /// Best visible bid level while reconstruction is enabled.
    ///
    /// # Errors
    /// Returns [`F3Error::QuantityArithmetic`] if a level cannot be summed exactly.
    pub fn best_bid(&self) -> Result<Option<MboLevel>, F3Error> {
        if !self.enabled {
            return Ok(None);
        }
        self.bids
            .iter()
            .next_back()
            .map(|(&price, ids)| self.level(PriceAtoms::new(price), ids))
            .transpose()
    }

    /// Best visible ask level while reconstruction is enabled.
    ///
    /// # Errors
    /// Returns [`F3Error::QuantityArithmetic`] if a level cannot be summed exactly.
    pub fn best_ask(&self) -> Result<Option<MboLevel>, F3Error> {
        if !self.enabled {
            return Ok(None);
        }
        self.asks
            .iter()
            .next()
            .map(|(&price, ids)| self.level(PriceAtoms::new(price), ids))
            .transpose()
    }

    /// Exact displayed quantity on one side.
    ///
    /// # Errors
    /// Returns [`F3Error::QuantityArithmetic`] on checked accumulation failure.
    pub fn total_quantity(&self, side: Side) -> Result<u128, F3Error> {
        if !self.enabled {
            return Ok(0);
        }
        self.orders
            .values()
            .filter(|order| order.side == side)
            .try_fold(0_u128, |total, order| {
                total
                    .checked_add(u128::from(order.quantity.get()))
                    .ok_or(F3Error::QuantityArithmetic)
            })
    }

    /// Atomically installs a complete snapshot and re-enables reconstruction.
    ///
    /// # Errors
    /// Returns [`F3Error::InvalidBook`] without changing prior state for malformed source facts.
    pub fn apply_snapshot(&mut self, snapshot: MboSnapshot) -> Result<MboApplyOutcome, F3Error> {
        let recovering = self.sequence.is_some();
        let mut next = Self::new();
        next.sequence = Some(snapshot.sequence);
        next.enabled = true;
        for order in snapshot.orders {
            Self::validate_visible_order(&order)?;
            if next.orders.contains_key(&order.source_order_id) {
                return Err(F3Error::InvalidBook);
            }
            next.push_queue(&order);
            next.orders.insert(order.source_order_id.clone(), order);
        }
        next.validate_uncrossed()?;
        *self = next;
        Ok(MboApplyOutcome {
            sequence: snapshot.sequence,
            uncertainty: if recovering {
                vec![F3Uncertainty::ReconnectPriorityRebuilt]
            } else {
                Vec::new()
            },
        })
    }

    /// Applies one exact next-sequence lifecycle event atomically.
    ///
    /// Source continuity or structural failure quarantines prior facts until recovery.
    ///
    /// # Errors
    /// Returns a stable [`F3Error`]. Continuity/structural errors disable the book.
    pub fn apply_event(&mut self, event: MboEvent) -> Result<MboApplyOutcome, F3Error> {
        if !self.enabled {
            return Err(F3Error::BookDisabled);
        }
        let Some(sequence) = self.sequence else {
            self.enabled = false;
            return Err(F3Error::SequenceGap);
        };
        if sequence.checked_add(1) != Some(event.sequence) {
            self.enabled = false;
            return Err(F3Error::SequenceGap);
        }

        let mut next = self.clone();
        let uncertainty = match next.apply_action(event.action) {
            Ok(value) => value,
            Err(error) => {
                self.enabled = false;
                return Err(error);
            }
        };
        if let Err(error) = next.validate_uncrossed() {
            self.enabled = false;
            return Err(error);
        }
        next.sequence = Some(event.sequence);
        *self = next;
        Ok(MboApplyOutcome {
            sequence: event.sequence,
            uncertainty,
        })
    }

    pub(super) fn queue(&self, side: Side, price: i64) -> Option<&Vec<String>> {
        match side {
            Side::Buy => self.bids.get(&price),
            Side::Sell => self.asks.get(&price),
        }
    }

    fn apply_action(&mut self, action: MboAction) -> Result<Vec<F3Uncertainty>, F3Error> {
        match action {
            MboAction::Add(order) => self.add(order),
            MboAction::Modify(order) => self.modify(order),
            MboAction::Cancel {
                source_order_id,
                side,
            } => self.cancel(&source_order_id, side),
            MboAction::Fill {
                source_order_id,
                side,
                quantity,
            } => self.fill(&source_order_id, side, quantity),
            MboAction::Clear { side } => {
                self.clear_side(side);
                Ok(Vec::new())
            }
        }
    }

    fn add(&mut self, order: VisibleOrder) -> Result<Vec<F3Uncertainty>, F3Error> {
        Self::validate_visible_order(&order)?;
        if self.orders.contains_key(&order.source_order_id) {
            return Err(F3Error::DuplicateSourceOrder);
        }
        self.push_queue(&order);
        self.orders.insert(order.source_order_id.clone(), order);
        Ok(Vec::new())
    }

    fn modify(&mut self, replacement: VisibleOrder) -> Result<Vec<F3Uncertainty>, F3Error> {
        Self::validate_visible_order(&replacement)?;
        let existing = self
            .orders
            .get(&replacement.source_order_id)
            .ok_or(F3Error::UnknownSourceOrder)?
            .clone();
        if existing.side != replacement.side {
            return Err(F3Error::SideMismatch);
        }
        if existing.price == replacement.price {
            let changed = existing.quantity != replacement.quantity;
            self.orders
                .insert(replacement.source_order_id.clone(), replacement);
            return Ok(if changed {
                vec![F3Uncertainty::SamePriceModifyPriorityAssumed]
            } else {
                Vec::new()
            });
        }
        self.remove_from_queue(
            existing.side,
            existing.price.get(),
            &existing.source_order_id,
        )?;
        self.push_queue(&replacement);
        self.orders
            .insert(replacement.source_order_id.clone(), replacement);
        Ok(Vec::new())
    }

    fn cancel(&mut self, source_order_id: &str, side: Side) -> Result<Vec<F3Uncertainty>, F3Error> {
        let existing = self
            .orders
            .get(source_order_id)
            .ok_or(F3Error::UnknownSourceOrder)?
            .clone();
        if existing.side != side {
            return Err(F3Error::SideMismatch);
        }
        self.remove_source_order(&existing)?;
        Ok(Vec::new())
    }

    fn fill(
        &mut self,
        source_order_id: &str,
        side: Side,
        quantity: QtyAtoms,
    ) -> Result<Vec<F3Uncertainty>, F3Error> {
        if quantity.get() == 0 {
            return Err(F3Error::InvalidBook);
        }
        let existing = self
            .orders
            .get(source_order_id)
            .ok_or(F3Error::UnknownSourceOrder)?
            .clone();
        if existing.side != side {
            return Err(F3Error::SideMismatch);
        }
        if quantity > existing.quantity {
            return Err(F3Error::SourceOverfill);
        }
        if quantity == existing.quantity {
            self.remove_source_order(&existing)?;
        } else {
            let order = self
                .orders
                .get_mut(source_order_id)
                .ok_or(F3Error::UnknownSourceOrder)?;
            order.quantity = QtyAtoms::new(existing.quantity.get() - quantity.get());
        }
        Ok(Vec::new())
    }

    fn validate_visible_order(order: &VisibleOrder) -> Result<(), F3Error> {
        if order.source_order_id.is_empty()
            || order.source_order_id.len() > MAX_SOURCE_ORDER_ID_BYTES
            || order.price.get() <= 0
            || order.quantity.get() == 0
        {
            return Err(F3Error::InvalidBook);
        }
        Ok(())
    }

    fn validate_uncrossed(&self) -> Result<(), F3Error> {
        if let (Some((&bid, _)), Some((&ask, _))) =
            (self.bids.iter().next_back(), self.asks.iter().next())
        {
            if bid >= ask {
                return Err(F3Error::InvalidBook);
            }
        }
        Ok(())
    }

    fn queue_mut(&mut self, side: Side) -> &mut BTreeMap<i64, Vec<String>> {
        match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        }
    }

    fn push_queue(&mut self, order: &VisibleOrder) {
        self.queue_mut(order.side)
            .entry(order.price.get())
            .or_default()
            .push(order.source_order_id.clone());
    }

    fn remove_source_order(&mut self, order: &VisibleOrder) -> Result<(), F3Error> {
        self.remove_from_queue(order.side, order.price.get(), &order.source_order_id)?;
        self.orders
            .remove(&order.source_order_id)
            .ok_or(F3Error::UnknownSourceOrder)?;
        Ok(())
    }

    fn remove_from_queue(
        &mut self,
        side: Side,
        price: i64,
        source_order_id: &str,
    ) -> Result<(), F3Error> {
        let queues = self.queue_mut(side);
        let ids = queues.get_mut(&price).ok_or(F3Error::InvalidBook)?;
        let position = ids
            .iter()
            .position(|candidate| candidate == source_order_id)
            .ok_or(F3Error::InvalidBook)?;
        ids.remove(position);
        if ids.is_empty() {
            queues.remove(&price);
        }
        Ok(())
    }

    fn clear_side(&mut self, side: Side) {
        self.orders.retain(|_, order| order.side != side);
        self.queue_mut(side).clear();
    }

    fn level(&self, price: PriceAtoms, ids: &[String]) -> Result<MboLevel, F3Error> {
        let quantity = ids.iter().try_fold(0_u64, |total, source_id| {
            let order = self.orders.get(source_id).ok_or(F3Error::InvalidBook)?;
            total
                .checked_add(order.quantity.get())
                .ok_or(F3Error::QuantityArithmetic)
        })?;
        Ok(MboLevel {
            price,
            quantity: QtyAtoms::new(quantity),
            order_count: ids.len(),
        })
    }
}
