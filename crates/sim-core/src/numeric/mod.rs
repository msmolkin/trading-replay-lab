//! Checked fixed-point primitives used by simulator accounting and matching.

#![allow(clippy::module_name_repetitions)]

use core::fmt;

/// Numeric failures are explicit; arithmetic never wraps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumericError {
    /// An operation exceeded its destination integer range.
    Overflow,
    /// A divisor was zero or negative where a positive scale divisor is required.
    InvalidDivisor,
    /// A decimal scale exceeded the supported canonical range.
    InvalidScale,
    /// A value that must be positive was zero or negative.
    NonPositive,
}

impl fmt::Display for NumericError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => formatter.write_str("fixed-point overflow"),
            Self::InvalidDivisor => formatter.write_str("invalid divisor"),
            Self::InvalidScale => formatter.write_str("invalid decimal scale"),
            Self::NonPositive => formatter.write_str("value must be positive"),
        }
    }
}

impl std::error::Error for NumericError {}

/// Decimal places carried by an integer quantity.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DecimalScale(u8);

impl DecimalScale {
    /// Canonical contracts currently allow at most eighteen decimal places.
    pub const MAX: u8 = 18;

    /// Creates a validated decimal scale.
    ///
    /// # Errors
    /// Returns [`NumericError::InvalidScale`] above [`Self::MAX`].
    pub fn new(value: u8) -> Result<Self, NumericError> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(NumericError::InvalidScale)
        }
    }

    /// Returns the decimal-place count.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Deterministic integer rounding policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rounding {
    /// Truncate the fractional remainder.
    TowardZero,
    /// Round any non-zero remainder away from zero.
    AwayFromZero,
    /// Round toward negative infinity.
    Floor,
    /// Round toward positive infinity.
    Ceiling,
    /// Round to nearest; exact halves go away from zero.
    NearestTiesAway,
}

/// Signed price atoms.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PriceAtoms(i64);

impl PriceAtoms {
    /// Creates a price atom value.
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Returns the raw atom count.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Checked addition.
    ///
    /// # Errors
    /// Returns [`NumericError::Overflow`] on overflow.
    pub fn checked_add(self, rhs: Self) -> Result<Self, NumericError> {
        self.0
            .checked_add(rhs.0)
            .map(Self)
            .ok_or(NumericError::Overflow)
    }
}

/// Unsigned quantity atoms.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct QtyAtoms(u64);

impl QtyAtoms {
    /// Creates a quantity atom value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw atom count.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Checked addition.
    ///
    /// # Errors
    /// Returns [`NumericError::Overflow`] on overflow.
    pub fn checked_add(self, rhs: Self) -> Result<Self, NumericError> {
        self.0
            .checked_add(rhs.0)
            .map(Self)
            .ok_or(NumericError::Overflow)
    }
}

/// Signed settlement-currency minor units.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MoneyMinor(i64);

impl MoneyMinor {
    /// Creates a monetary minor-unit value.
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Returns the raw minor-unit count.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Checked addition.
    ///
    /// # Errors
    /// Returns [`NumericError::Overflow`] on overflow.
    pub fn checked_add(self, rhs: Self) -> Result<Self, NumericError> {
        self.0
            .checked_add(rhs.0)
            .map(Self)
            .ok_or(NumericError::Overflow)
    }
}

/// Signed parts-per-billion rate.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RatePpb(i64);

impl RatePpb {
    /// Creates a parts-per-billion rate.
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Returns the raw rate.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

fn pow10(scale: u8) -> Result<i128, NumericError> {
    if scale > DecimalScale::MAX {
        return Err(NumericError::InvalidScale);
    }
    Ok(10_i128.pow(u32::from(scale)))
}

fn div_round(numerator: i128, denominator: i128, rounding: Rounding) -> Result<i128, NumericError> {
    if denominator <= 0 {
        return Err(NumericError::InvalidDivisor);
    }
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder == 0 {
        return Ok(quotient);
    }
    let sign = if numerator.is_negative() { -1 } else { 1 };
    let rounded = match rounding {
        Rounding::AwayFromZero => quotient.checked_add(sign),
        Rounding::Floor if numerator.is_negative() => quotient.checked_sub(1),
        Rounding::Ceiling if numerator.is_positive() => quotient.checked_add(1),
        Rounding::TowardZero | Rounding::Floor | Rounding::Ceiling => Some(quotient),
        Rounding::NearestTiesAway => {
            let doubled = remainder
                .abs()
                .checked_mul(2)
                .ok_or(NumericError::Overflow)?;
            if doubled >= denominator {
                quotient.checked_add(sign)
            } else {
                Some(quotient)
            }
        }
    };
    rounded.ok_or(NumericError::Overflow)
}

/// Rescales a signed integer between decimal scales using an explicit rounding policy.
///
/// # Errors
/// Returns an error for invalid scales or overflow.
pub fn rescale_i64(
    value: i64,
    from_scale: DecimalScale,
    to_scale: DecimalScale,
    rounding: Rounding,
) -> Result<i64, NumericError> {
    let wide = i128::from(value);
    let adjusted = match from_scale.get().cmp(&to_scale.get()) {
        core::cmp::Ordering::Equal => wide,
        core::cmp::Ordering::Less => {
            let factor = pow10(to_scale.get() - from_scale.get())?;
            wide.checked_mul(factor).ok_or(NumericError::Overflow)?
        }
        core::cmp::Ordering::Greater => {
            let factor = pow10(from_scale.get() - to_scale.get())?;
            div_round(wide, factor, rounding)?
        }
    };
    i64::try_from(adjusted).map_err(|_| NumericError::Overflow)
}

/// Computes linear-contract notional in settlement-currency minor units.
///
/// The exact integer product is `qty_atoms * price_atoms * multiplier_atoms`; its source
/// decimal scale is the sum of the three component scales. The final rescale is explicit.
///
/// # Errors
/// Returns an error for invalid scales, intermediate overflow, or an `i64` result overflow.
#[allow(clippy::too_many_arguments)]
pub fn linear_notional_minor(
    qty: QtyAtoms,
    price: PriceAtoms,
    contract_multiplier_atoms: u64,
    qty_scale: DecimalScale,
    price_scale: DecimalScale,
    multiplier_scale: DecimalScale,
    settlement_scale: DecimalScale,
    rounding: Rounding,
) -> Result<MoneyMinor, NumericError> {
    if contract_multiplier_atoms == 0 {
        return Err(NumericError::NonPositive);
    }
    let total_scale = qty_scale
        .get()
        .checked_add(price_scale.get())
        .and_then(|value| value.checked_add(multiplier_scale.get()))
        .ok_or(NumericError::InvalidScale)?;
    let product = i128::from(qty.get())
        .checked_mul(i128::from(price.get()))
        .and_then(|value| value.checked_mul(i128::from(contract_multiplier_atoms)))
        .ok_or(NumericError::Overflow)?;
    let adjusted = if total_scale >= settlement_scale.get() {
        div_round(
            product,
            pow10(total_scale - settlement_scale.get())?,
            rounding,
        )?
    } else {
        product
            .checked_mul(pow10(settlement_scale.get() - total_scale)?)
            .ok_or(NumericError::Overflow)?
    };
    i64::try_from(adjusted)
        .map(MoneyMinor::new)
        .map_err(|_| NumericError::Overflow)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]

    use super::*;

    const BASE: u128 = 1_000_000_000;

    #[derive(Clone, Debug)]
    struct BigUnsigned(Vec<u32>);

    impl BigUnsigned {
        fn from_u64(mut value: u64) -> Self {
            let mut limbs = Vec::new();
            while value != 0 {
                limbs.push((u128::from(value) % BASE) as u32);
                value = (u128::from(value) / BASE) as u64;
            }
            Self(limbs)
        }

        fn mul_u64(&mut self, multiplier: u64) {
            let mut carry = 0_u128;
            for limb in &mut self.0 {
                let product = u128::from(*limb) * u128::from(multiplier) + carry;
                *limb = (product % BASE) as u32;
                carry = product / BASE;
            }
            while carry != 0 {
                self.0.push((carry % BASE) as u32);
                carry /= BASE;
            }
        }

        fn div_u64(&mut self, divisor: u64) -> u64 {
            let mut remainder = 0_u128;
            for limb in self.0.iter_mut().rev() {
                let current = remainder * BASE + u128::from(*limb);
                *limb = (current / u128::from(divisor)) as u32;
                remainder = current % u128::from(divisor);
            }
            while self.0.last() == Some(&0) {
                self.0.pop();
            }
            remainder as u64
        }

        fn to_u64(&self) -> Option<u64> {
            let mut value = 0_u128;
            for limb in self.0.iter().rev() {
                value = value.checked_mul(BASE)?.checked_add(u128::from(*limb))?;
            }
            u64::try_from(value).ok()
        }
    }

    #[test]
    fn rescale_rounding_is_signed_and_explicit() {
        let two = DecimalScale::new(2).unwrap();
        let zero = DecimalScale::new(0).unwrap();
        assert_eq!(rescale_i64(155, two, zero, Rounding::TowardZero), Ok(1));
        assert_eq!(
            rescale_i64(155, two, zero, Rounding::NearestTiesAway),
            Ok(2)
        );
        assert_eq!(rescale_i64(-155, two, zero, Rounding::Floor), Ok(-2));
        assert_eq!(rescale_i64(-155, two, zero, Rounding::Ceiling), Ok(-1));
    }

    #[test]
    fn arithmetic_overflow_fails_closed() {
        assert_eq!(
            PriceAtoms::new(i64::MAX).checked_add(PriceAtoms::new(1)),
            Err(NumericError::Overflow)
        );
        assert_eq!(
            QtyAtoms::new(u64::MAX).checked_add(QtyAtoms::new(1)),
            Err(NumericError::Overflow)
        );
        assert_eq!(
            MoneyMinor::new(i64::MIN).checked_add(MoneyMinor::new(-1)),
            Err(NumericError::Overflow)
        );
    }

    #[test]
    fn linear_notional_matches_arbitrary_precision_reference() {
        let qty_scale = DecimalScale::new(3).unwrap();
        let price_scale = DecimalScale::new(2).unwrap();
        let multiplier_scale = DecimalScale::new(1).unwrap();
        let settlement_scale = DecimalScale::new(2).unwrap();
        let divisor = 10_u64.pow(4);
        let mut state = 0x4d59_5df4_d0f3_3173_u64;
        for _ in 0..512 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let qty = state % 1_000_000 + 1;
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let price = (state % 10_000_000 + 1).cast_signed();
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let multiplier = state % 10_000 + 1;

            let actual = linear_notional_minor(
                QtyAtoms::new(qty),
                PriceAtoms::new(price),
                multiplier,
                qty_scale,
                price_scale,
                multiplier_scale,
                settlement_scale,
                Rounding::TowardZero,
            )
            .unwrap()
            .get();

            let mut reference = BigUnsigned::from_u64(qty);
            reference.mul_u64(price.cast_unsigned());
            reference.mul_u64(multiplier);
            reference.div_u64(divisor);
            let expected = reference.to_u64().unwrap();
            assert_eq!(u64::try_from(actual).unwrap(), expected);
        }
    }
}
