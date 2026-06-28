//! Signed fixed-point `Price`.

use serde::{Deserialize, Serialize};

use crate::error::ArithmeticError;

/// A signed, fixed-point price carried as a raw `i128` scaled integer.
///
/// `Price` is precision-free: the scale (tick size) is instrument metadata, not
/// stored here. Negative prices are valid (spreads, basis instruments).
///
/// # Ordering invariant
///
/// Comparison is meaningful **only** among prices of the same instrument and
/// precision. `Ord` is derived because same-instrument book and limit logic needs
/// it, but comparing prices of different instruments compiles and means nothing.
///
/// ```
/// use oath_model::Price;
/// let p = Price::from_raw(12_345);
/// assert_eq!(p.raw(), 12_345);
/// assert_eq!(Price::from_raw(10).checked_add(Price::from_raw(5)), Ok(Price::from_raw(15)));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Price(i128);

impl Price {
    /// Wraps a raw scaled integer as a `Price`.
    #[must_use]
    pub const fn from_raw(raw: i128) -> Self {
        Self(raw)
    }

    /// Returns the raw scaled integer.
    #[must_use]
    pub const fn raw(self) -> i128 {
        self.0
    }

    /// Adds two prices, returning an error on overflow or underflow rather than
    /// wrapping or panicking.
    ///
    /// # Errors
    ///
    /// Returns [`ArithmeticError::Overflow`] or [`ArithmeticError::Underflow`] if
    /// the result is out of `i128` range.
    pub const fn checked_add(self, rhs: Self) -> Result<Self, ArithmeticError> {
        match self.0.checked_add(rhs.0) {
            Some(v) => Ok(Self(v)),
            None if self.0 > 0 => Err(ArithmeticError::Overflow),
            None => Err(ArithmeticError::Underflow),
        }
    }

    /// Subtracts one price from another, returning an error on overflow or
    /// underflow rather than wrapping or panicking.
    ///
    /// # Errors
    ///
    /// Returns [`ArithmeticError::Overflow`] or [`ArithmeticError::Underflow`] if
    /// the result is out of `i128` range.
    pub const fn checked_sub(self, rhs: Self) -> Result<Self, ArithmeticError> {
        match self.0.checked_sub(rhs.0) {
            Some(v) => Ok(Self(v)),
            None if rhs.0 < 0 => Err(ArithmeticError::Overflow),
            None => Err(ArithmeticError::Underflow),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Price;
    use crate::error::ArithmeticError;
    use proptest::prelude::*;

    #[test]
    fn add_and_sub_happy_path() {
        assert_eq!(
            Price::from_raw(10).checked_add(Price::from_raw(5)),
            Ok(Price::from_raw(15))
        );
        assert_eq!(
            Price::from_raw(10).checked_sub(Price::from_raw(5)),
            Ok(Price::from_raw(5))
        );
    }

    #[test]
    fn add_overflow_and_underflow() {
        assert_eq!(
            Price::from_raw(i128::MAX).checked_add(Price::from_raw(1)),
            Err(ArithmeticError::Overflow)
        );
        assert_eq!(
            Price::from_raw(i128::MIN).checked_add(Price::from_raw(-1)),
            Err(ArithmeticError::Underflow)
        );
    }

    #[test]
    fn sub_min_boundary() {
        assert_eq!(
            Price::from_raw(0).checked_sub(Price::from_raw(i128::MIN)),
            Err(ArithmeticError::Overflow)
        );
        assert_eq!(
            Price::from_raw(i128::MIN).checked_sub(Price::from_raw(1)),
            Err(ArithmeticError::Underflow)
        );
    }

    #[test]
    fn ordering_matches_integers() {
        assert!(Price::from_raw(-1) < Price::from_raw(0));
        assert!(Price::from_raw(0) < Price::from_raw(1));
    }

    proptest! {
        #[test]
        fn raw_round_trip(x in any::<i128>()) {
            prop_assert_eq!(Price::from_raw(x).raw(), x);
        }

        #[test]
        fn add_sub_inverse(a in any::<i128>(), b in any::<i128>()) {
            let pa = Price::from_raw(a);
            let pb = Price::from_raw(b);
            if let Ok(sum) = pa.checked_add(pb) {
                prop_assert_eq!(sum.checked_sub(pb), Ok(pa));
            }
        }

        #[test]
        fn add_commutative(a in any::<i128>(), b in any::<i128>()) {
            prop_assert_eq!(
                Price::from_raw(a).checked_add(Price::from_raw(b)),
                Price::from_raw(b).checked_add(Price::from_raw(a))
            );
        }

        #[test]
        fn json_round_trip(x in any::<i128>()) {
            let p = Price::from_raw(x);
            let back = serde_json::to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str::<Price>(&s).ok());
            prop_assert_eq!(back, Some(p));
        }
    }
}
