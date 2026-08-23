//! Deterministic position basis, realized P&L, and reversal accounting.

use core::fmt;

use crate::numeric::{
    DecimalScale, MoneyMinor, NumericError, PriceAtoms, QtyAtoms, Rounding, linear_notional_minor,
};

/// Contract math needed to turn quantity/price atoms into settlement minor units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PositionMath {
    /// Integer contract multiplier.
    pub contract_multiplier_atoms: u64,
    /// Quantity decimal scale.
    pub qty_scale: DecimalScale,
    /// Price decimal scale.
    pub price_scale: DecimalScale,
    /// Contract-multiplier decimal scale.
    pub multiplier_scale: DecimalScale,
    /// Settlement-currency decimal scale.
    pub settlement_scale: DecimalScale,
    /// Explicit rounding policy for settlement-money conversion.
    pub rounding: Rounding,
}

/// Direction of an execution fill.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FillSide {
    /// Buy quantity.
    Buy,
    /// Sell quantity.
    Sell,
}

/// Stable position-accounting failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionError {
    /// Fill quantity must be positive.
    ZeroQuantity,
    /// Fill price must be positive.
    InvalidPrice,
    /// Quantity cannot be represented by the signed position type.
    QuantityOverflow,
    /// Existing state violates position invariants.
    InvalidState,
    /// Checked arithmetic failed.
    Numeric(NumericError),
}

impl fmt::Display for PositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroQuantity => formatter.write_str("fill quantity must be positive"),
            Self::InvalidPrice => formatter.write_str("fill price must be positive"),
            Self::QuantityOverflow => formatter.write_str("position quantity overflow"),
            Self::InvalidState => formatter.write_str("invalid position state"),
            Self::Numeric(error) => write!(formatter, "position arithmetic failed: {error}"),
        }
    }
}

impl std::error::Error for PositionError {}

impl From<NumericError> for PositionError {
    fn from(value: NumericError) -> Self {
        Self::Numeric(value)
    }
}

/// Kind of accounting leg created by one execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PositionLegKind {
    /// Quantity opened or added to the resulting position.
    Open,
    /// Quantity closed against the prior position basis.
    Close,
}

/// One explicit accounting leg from a fill.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PositionLeg {
    /// Whether this leg opens or closes exposure.
    pub kind: PositionLegKind,
    /// Signed quantity change; buy is positive and sell is negative.
    pub signed_qty_atoms: i64,
    /// Execution price for this leg.
    pub price: PriceAtoms,
    /// Realized P&L caused by this leg.
    pub realized_pnl_delta: MoneyMinor,
}

/// Authoritative economic position snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Position {
    /// Signed base/contract quantity; positive is long and negative is short.
    pub quantity_atoms: i64,
    /// Average entry basis for non-zero positions.
    pub average_entry_price: Option<PriceAtoms>,
    /// Cumulative realized settlement P&L.
    pub realized_pnl: MoneyMinor,
}

impl Default for Position {
    fn default() -> Self {
        Self::flat()
    }
}

impl Position {
    /// Creates a flat zero-P&L position.
    #[must_use]
    pub const fn flat() -> Self {
        Self {
            quantity_atoms: 0,
            average_entry_price: None,
            realized_pnl: MoneyMinor::new(0),
        }
    }

    /// Restores a position only when basis invariants are valid.
    ///
    /// # Errors
    /// Returns [`PositionError::InvalidState`] when flat/non-flat basis does not match.
    pub fn from_snapshot(snapshot: Self) -> Result<Self, PositionError> {
        snapshot.validate()?;
        Ok(snapshot)
    }

    /// Applies one fill atomically and returns its one or two accounting legs.
    ///
    /// Reversals are represented as a close leg through zero followed by a new open leg
    /// at the execution price. The prior average basis is never carried across zero.
    ///
    /// # Errors
    /// Returns a stable validation or checked-arithmetic error without mutating `self`.
    pub fn apply_fill(
        &mut self,
        side: FillSide,
        quantity: QtyAtoms,
        price: PriceAtoms,
        math: PositionMath,
    ) -> Result<Vec<PositionLeg>, PositionError> {
        self.validate()?;
        if quantity.get() == 0 {
            return Err(PositionError::ZeroQuantity);
        }
        if price.get() <= 0 {
            return Err(PositionError::InvalidPrice);
        }
        let fill_abs = i64::try_from(quantity.get()).map_err(|_| PositionError::QuantityOverflow)?;
        let signed_fill = match side {
            FillSide::Buy => fill_abs,
            FillSide::Sell => fill_abs.checked_neg().ok_or(PositionError::QuantityOverflow)?,
        };

        let mut next = *self;
        let legs = next.apply_validated_fill(signed_fill, quantity, price, math)?;
        next.validate()?;
        *self = next;
        Ok(legs)
    }

    fn validate(&self) -> Result<(), PositionError> {
        match (self.quantity_atoms == 0, self.average_entry_price) {
            (true, None) => Ok(()),
            (false, Some(price)) if price.get() > 0 => Ok(()),
            _ => Err(PositionError::InvalidState),
        }
    }

    fn apply_validated_fill(
        &mut self,
        signed_fill: i64,
        quantity: QtyAtoms,
        price: PriceAtoms,
        math: PositionMath,
    ) -> Result<Vec<PositionLeg>, PositionError> {
        if self.quantity_atoms == 0 || self.quantity_atoms.signum() == signed_fill.signum() {
            let old_abs = self.quantity_atoms.unsigned_abs();
            let new_abs = old_abs
                .checked_add(quantity.get())
                .ok_or(PositionError::QuantityOverflow)?;
            let new_signed = signed_quantity(new_abs, signed_fill.is_positive())?;
            self.average_entry_price = Some(if old_abs == 0 {
                price
            } else {
                weighted_average(
                    old_abs,
                    self.average_entry_price.ok_or(PositionError::InvalidState)?,
                    quantity.get(),
                    price,
                )?
            });
            self.quantity_atoms = new_signed;
            return Ok(vec![PositionLeg {
                kind: PositionLegKind::Open,
                signed_qty_atoms: signed_fill,
                price,
                realized_pnl_delta: MoneyMinor::new(0),
            }]);
        }

        let old_qty = self.quantity_atoms;
        let old_abs = old_qty.unsigned_abs();
        let close_abs = old_abs.min(quantity.get());
        let close_qty = QtyAtoms::new(close_abs);
        let basis = self.average_entry_price.ok_or(PositionError::InvalidState)?;
        let realized = realized_close_pnl(close_qty, basis, price, old_qty.is_positive(), math)?;
        self.realized_pnl = self.realized_pnl.checked_add(realized)?;

        let close_signed = signed_quantity(close_abs, signed_fill.is_positive())?;
        let mut legs = vec![PositionLeg {
            kind: PositionLegKind::Close,
            signed_qty_atoms: close_signed,
            price,
            realized_pnl_delta: realized,
        }];

        match quantity.get().cmp(&old_abs) {
            core::cmp::Ordering::Less => {
                self.quantity_atoms = old_qty
                    .checked_add(close_signed)
                    .ok_or(PositionError::QuantityOverflow)?;
            }
            core::cmp::Ordering::Equal => {
                self.quantity_atoms = 0;
                self.average_entry_price = None;
            }
            core::cmp::Ordering::Greater => {
                let open_abs = quantity.get() - old_abs;
                let open_signed = signed_quantity(open_abs, signed_fill.is_positive())?;
                self.quantity_atoms = open_signed;
                self.average_entry_price = Some(price);
                legs.push(PositionLeg {
                    kind: PositionLegKind::Open,
                    signed_qty_atoms: open_signed,
                    price,
                    realized_pnl_delta: MoneyMinor::new(0),
                });
            }
        }
        Ok(legs)
    }
}

fn signed_quantity(value: u64, positive: bool) -> Result<i64, PositionError> {
    let signed = i64::try_from(value).map_err(|_| PositionError::QuantityOverflow)?;
    if positive {
        Ok(signed)
    } else {
        signed.checked_neg().ok_or(PositionError::QuantityOverflow)
    }
}

fn weighted_average(
    old_qty: u64,
    old_price: PriceAtoms,
    add_qty: u64,
    add_price: PriceAtoms,
) -> Result<PriceAtoms, PositionError> {
    let denominator = old_qty
        .checked_add(add_qty)
        .ok_or(PositionError::QuantityOverflow)?;
    let numerator = i128::from(old_qty)
        .checked_mul(i128::from(old_price.get()))
        .and_then(|value| {
            i128::from(add_qty)
                .checked_mul(i128::from(add_price.get()))
                .and_then(|added| value.checked_add(added))
        })
        .ok_or(PositionError::Numeric(NumericError::Overflow))?;
    let average = numerator / i128::from(denominator);
    i64::try_from(average)
        .map(PriceAtoms::new)
        .map_err(|_| PositionError::Numeric(NumericError::Overflow))
}

fn realized_close_pnl(
    quantity: QtyAtoms,
    entry: PriceAtoms,
    exit: PriceAtoms,
    was_long: bool,
    math: PositionMath,
) -> Result<MoneyMinor, PositionError> {
    let entry_value = linear_notional_minor(
        quantity,
        entry,
        math.contract_multiplier_atoms,
        math.qty_scale,
        math.price_scale,
        math.multiplier_scale,
        math.settlement_scale,
        math.rounding,
    )?;
    let exit_value = linear_notional_minor(
        quantity,
        exit,
        math.contract_multiplier_atoms,
        math.qty_scale,
        math.price_scale,
        math.multiplier_scale,
        math.settlement_scale,
        math.rounding,
    )?;
    let value = if was_long {
        exit_value.get().checked_sub(entry_value.get())
    } else {
        entry_value.get().checked_sub(exit_value.get())
    }
    .ok_or(PositionError::Numeric(NumericError::Overflow))?;
    Ok(MoneyMinor::new(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn math() -> PositionMath {
        PositionMath {
            contract_multiplier_atoms: 1,
            qty_scale: DecimalScale::new(0).unwrap(),
            price_scale: DecimalScale::new(0).unwrap(),
            multiplier_scale: DecimalScale::new(0).unwrap(),
            settlement_scale: DecimalScale::new(0).unwrap(),
            rounding: Rounding::TowardZero,
        }
    }

    #[test]
    fn adds_use_deterministic_weighted_average() {
        let mut position = Position::flat();
        position
            .apply_fill(
                FillSide::Buy,
                QtyAtoms::new(2),
                PriceAtoms::new(100),
                math(),
            )
            .unwrap();
        position
            .apply_fill(
                FillSide::Buy,
                QtyAtoms::new(1),
                PriceAtoms::new(103),
                math(),
            )
            .unwrap();
        assert_eq!(position.quantity_atoms, 3);
        assert_eq!(position.average_entry_price, Some(PriceAtoms::new(101)));
        assert_eq!(position.realized_pnl, MoneyMinor::new(0));
    }

    #[test]
    fn partial_close_keeps_basis_and_realizes_exactly() {
        let mut position = Position::flat();
        position
            .apply_fill(
                FillSide::Buy,
                QtyAtoms::new(10),
                PriceAtoms::new(100),
                math(),
            )
            .unwrap();
        let legs = position
            .apply_fill(
                FillSide::Sell,
                QtyAtoms::new(4),
                PriceAtoms::new(110),
                math(),
            )
            .unwrap();
        assert_eq!(legs.len(), 1);
        assert_eq!(legs[0].kind, PositionLegKind::Close);
        assert_eq!(legs[0].realized_pnl_delta, MoneyMinor::new(40));
        assert_eq!(position.quantity_atoms, 6);
        assert_eq!(position.average_entry_price, Some(PriceAtoms::new(100)));
        assert_eq!(position.realized_pnl, MoneyMinor::new(40));
    }

    #[test]
    fn long_to_short_reversal_is_two_legs_and_resets_basis() {
        let mut position = Position::flat();
        position
            .apply_fill(
                FillSide::Buy,
                QtyAtoms::new(5),
                PriceAtoms::new(100),
                math(),
            )
            .unwrap();
        let legs = position
            .apply_fill(
                FillSide::Sell,
                QtyAtoms::new(8),
                PriceAtoms::new(110),
                math(),
            )
            .unwrap();
        assert_eq!(legs.len(), 2);
        assert_eq!(legs[0].kind, PositionLegKind::Close);
        assert_eq!(legs[0].signed_qty_atoms, -5);
        assert_eq!(legs[0].realized_pnl_delta, MoneyMinor::new(50));
        assert_eq!(legs[1].kind, PositionLegKind::Open);
        assert_eq!(legs[1].signed_qty_atoms, -3);
        assert_eq!(position.quantity_atoms, -3);
        assert_eq!(position.average_entry_price, Some(PriceAtoms::new(110)));
    }

    #[test]
    fn short_to_long_reversal_realizes_short_pnl() {
        let mut position = Position::flat();
        position
            .apply_fill(
                FillSide::Sell,
                QtyAtoms::new(4),
                PriceAtoms::new(120),
                math(),
            )
            .unwrap();
        let legs = position
            .apply_fill(
                FillSide::Buy,
                QtyAtoms::new(6),
                PriceAtoms::new(100),
                math(),
            )
            .unwrap();
        assert_eq!(legs[0].realized_pnl_delta, MoneyMinor::new(80));
        assert_eq!(position.quantity_atoms, 2);
        assert_eq!(position.average_entry_price, Some(PriceAtoms::new(100)));
        assert_eq!(position.realized_pnl, MoneyMinor::new(80));
    }

    #[test]
    fn invalid_fill_is_atomic() {
        let mut position = Position::flat();
        let before = position;
        assert_eq!(
            position.apply_fill(
                FillSide::Buy,
                QtyAtoms::new(1),
                PriceAtoms::new(0),
                math(),
            ),
            Err(PositionError::InvalidPrice)
        );
        assert_eq!(position, before);
    }

    #[test]
    fn flat_snapshot_cannot_carry_basis() {
        let invalid = Position {
            quantity_atoms: 0,
            average_entry_price: Some(PriceAtoms::new(100)),
            realized_pnl: MoneyMinor::new(0),
        };
        assert_eq!(Position::from_snapshot(invalid), Err(PositionError::InvalidState));
    }
}
