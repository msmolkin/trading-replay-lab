//! Point-in-time instrument definitions and increment validation.

#![allow(clippy::module_name_repetitions)]

use core::fmt;

use crate::numeric::{
    DecimalScale, MoneyMinor, NumericError, PriceAtoms, QtyAtoms, Rounding, linear_notional_minor,
};

/// Supported broad asset classes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetClass {
    /// Spot or derivative cryptoasset.
    Crypto,
    /// Listed equity.
    Equity,
    /// Listed futures contract.
    Future,
}

/// Canonical product type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProductType {
    /// Spot instrument.
    Spot,
    /// Perpetual derivative.
    Perpetual,
    /// Dated future.
    Future,
    /// Equity share.
    Equity,
}

/// Settlement formula family.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettlementKind {
    /// Linear quote/settlement calculation.
    Linear,
    /// Inverse calculation, intentionally delegated to a later calculator.
    Inverse,
}

/// Instrument-definition validation failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InstrumentError {
    /// Required stable identifier is empty.
    EmptyIdentifier(&'static str),
    /// Tick or quantity increment is zero.
    ZeroIncrement(&'static str),
    /// Contract multiplier is zero.
    ZeroMultiplier,
    /// Effective interval is empty or reversed.
    InvalidEffectiveInterval,
    /// Listing/expiry metadata is inconsistent.
    InvalidLifecycle,
    /// Price is non-positive or off tick.
    InvalidPrice,
    /// Quantity is zero or off increment.
    InvalidQuantity,
    /// Instrument definition is not active at the requested instant.
    InactiveDefinition,
    /// Requested calculator does not apply to this settlement kind.
    UnsupportedSettlement,
    /// Numeric calculation failed.
    Numeric(NumericError),
}

impl fmt::Display for InstrumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyIdentifier(field) => {
                write!(formatter, "empty instrument identifier: {field}")
            }
            Self::ZeroIncrement(field) => write!(formatter, "zero instrument increment: {field}"),
            Self::ZeroMultiplier => formatter.write_str("zero contract multiplier"),
            Self::InvalidEffectiveInterval => {
                formatter.write_str("invalid definition effective interval")
            }
            Self::InvalidLifecycle => formatter.write_str("invalid listing or expiry interval"),
            Self::InvalidPrice => formatter.write_str("price is non-positive or not tick aligned"),
            Self::InvalidQuantity => {
                formatter.write_str("quantity is zero or not increment aligned")
            }
            Self::InactiveDefinition => formatter.write_str("instrument definition is not active"),
            Self::UnsupportedSettlement => formatter.write_str("unsupported settlement calculator"),
            Self::Numeric(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for InstrumentError {}

impl From<NumericError> for InstrumentError {
    fn from(value: NumericError) -> Self {
        Self::Numeric(value)
    }
}

/// Canonical point-in-time instrument definition used by the simulator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstrumentDefinition {
    /// Stable canonical instrument identifier.
    pub instrument_id: String,
    /// Canonical venue identifier.
    pub venue_id: String,
    /// Broad asset class.
    pub asset_class: AssetClass,
    /// Product type.
    pub product_type: ProductType,
    /// Price decimal scale.
    pub price_scale: DecimalScale,
    /// Quantity decimal scale.
    pub qty_scale: DecimalScale,
    /// Minimum price increment in atoms.
    pub tick_size_atoms: u64,
    /// Minimum quantity increment in atoms.
    pub qty_increment_atoms: u64,
    /// Contract multiplier in fixed-point atoms.
    pub contract_multiplier_atoms: u64,
    /// Contract-multiplier decimal scale.
    pub multiplier_scale: DecimalScale,
    /// Settlement formula family.
    pub settlement_kind: SettlementKind,
    /// Optional listing instant, inclusive.
    pub listing_ns: Option<i64>,
    /// Optional expiry instant, exclusive.
    pub expiry_ns: Option<i64>,
    /// Definition-effective instant, inclusive.
    pub effective_from_ns: i64,
    /// Definition-effective instant, exclusive when present.
    pub effective_through_ns: Option<i64>,
}

impl InstrumentDefinition {
    /// Validates structural and lifecycle invariants.
    ///
    /// # Errors
    /// Returns an [`InstrumentError`] when an identifier, increment, multiplier, or interval is invalid.
    pub fn validate(&self) -> Result<(), InstrumentError> {
        if self.instrument_id.trim().is_empty() {
            return Err(InstrumentError::EmptyIdentifier("instrument_id"));
        }
        if self.venue_id.trim().is_empty() {
            return Err(InstrumentError::EmptyIdentifier("venue_id"));
        }
        if self.tick_size_atoms == 0 {
            return Err(InstrumentError::ZeroIncrement("tick_size_atoms"));
        }
        if self.qty_increment_atoms == 0 {
            return Err(InstrumentError::ZeroIncrement("qty_increment_atoms"));
        }
        if self.contract_multiplier_atoms == 0 {
            return Err(InstrumentError::ZeroMultiplier);
        }
        if self
            .effective_through_ns
            .is_some_and(|through| through <= self.effective_from_ns)
        {
            return Err(InstrumentError::InvalidEffectiveInterval);
        }
        if matches!(
            (self.listing_ns, self.expiry_ns),
            (Some(listing), Some(expiry)) if expiry <= listing
        ) {
            return Err(InstrumentError::InvalidLifecycle);
        }
        Ok(())
    }

    /// Returns whether this exact definition is active at `ts_event_ns`.
    #[must_use]
    pub fn is_active_at(&self, ts_event_ns: i64) -> bool {
        let effective = ts_event_ns >= self.effective_from_ns
            && self
                .effective_through_ns
                .is_none_or(|through| ts_event_ns < through);
        let listed = self.listing_ns.is_none_or(|listing| ts_event_ns >= listing);
        let unexpired = self.expiry_ns.is_none_or(|expiry| ts_event_ns < expiry);
        effective && listed && unexpired
    }

    /// Validates a price/quantity pair at a point in time.
    ///
    /// # Errors
    /// Returns an [`InstrumentError`] when the definition is inactive or values violate increments.
    pub fn validate_trade_values(
        &self,
        price: PriceAtoms,
        qty: QtyAtoms,
        ts_event_ns: i64,
    ) -> Result<(), InstrumentError> {
        self.validate()?;
        if !self.is_active_at(ts_event_ns) {
            return Err(InstrumentError::InactiveDefinition);
        }
        let price_value = price.get();
        if price_value <= 0 {
            return Err(InstrumentError::InvalidPrice);
        }
        let price_unsigned =
            u64::try_from(price_value).map_err(|_| InstrumentError::InvalidPrice)?;
        if price_unsigned % self.tick_size_atoms != 0 {
            return Err(InstrumentError::InvalidPrice);
        }
        if qty.get() == 0 || qty.get() % self.qty_increment_atoms != 0 {
            return Err(InstrumentError::InvalidQuantity);
        }
        Ok(())
    }

    /// Computes linear notional for this instrument.
    ///
    /// # Errors
    /// Returns [`InstrumentError::UnsupportedSettlement`] for inverse instruments and propagates numeric overflow.
    pub fn linear_notional(
        &self,
        qty: QtyAtoms,
        price: PriceAtoms,
        settlement_scale: DecimalScale,
        rounding: Rounding,
    ) -> Result<MoneyMinor, InstrumentError> {
        if self.settlement_kind != SettlementKind::Linear {
            return Err(InstrumentError::UnsupportedSettlement);
        }
        linear_notional_minor(
            qty,
            price,
            self.contract_multiplier_atoms,
            self.qty_scale,
            self.price_scale,
            self.multiplier_scale,
            settlement_scale,
            rounding,
        )
        .map_err(InstrumentError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instrument() -> InstrumentDefinition {
        InstrumentDefinition {
            instrument_id: "SYNTH-BTC-USD".into(),
            venue_id: "TRL-SYNTH".into(),
            asset_class: AssetClass::Crypto,
            product_type: ProductType::Perpetual,
            price_scale: DecimalScale::new(2).unwrap(),
            qty_scale: DecimalScale::new(0).unwrap(),
            tick_size_atoms: 10,
            qty_increment_atoms: 2,
            contract_multiplier_atoms: 1,
            multiplier_scale: DecimalScale::new(0).unwrap(),
            settlement_kind: SettlementKind::Linear,
            listing_ns: Some(100),
            expiry_ns: Some(1_000),
            effective_from_ns: 100,
            effective_through_ns: Some(900),
        }
    }

    #[test]
    fn point_in_time_validation_is_half_open() {
        let definition = instrument();
        assert!(!definition.is_active_at(99));
        assert!(definition.is_active_at(100));
        assert!(definition.is_active_at(899));
        assert!(!definition.is_active_at(900));
    }

    #[test]
    fn increments_fail_closed() {
        let definition = instrument();
        assert_eq!(
            definition.validate_trade_values(PriceAtoms::new(10_005), QtyAtoms::new(2), 200),
            Err(InstrumentError::InvalidPrice)
        );
        assert_eq!(
            definition.validate_trade_values(PriceAtoms::new(10_000), QtyAtoms::new(3), 200),
            Err(InstrumentError::InvalidQuantity)
        );
        assert_eq!(
            definition.validate_trade_values(PriceAtoms::new(10_000), QtyAtoms::new(2), 900),
            Err(InstrumentError::InactiveDefinition)
        );
    }

    #[test]
    fn linear_notional_uses_declared_scales() {
        let definition = instrument();
        let cents = DecimalScale::new(2).unwrap();
        assert_eq!(
            definition
                .linear_notional(
                    QtyAtoms::new(3),
                    PriceAtoms::new(12_345),
                    cents,
                    Rounding::TowardZero,
                )
                .unwrap(),
            MoneyMinor::new(37_035)
        );
    }
}
