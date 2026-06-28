//! Unsigned fixed-point `Quantity`.

use serde::{Deserialize, Serialize};

use crate::error::ArithmeticError;

/// An unsigned, fixed-point quantity carried as a raw `u128` scaled integer.
///
/// A `Quantity` is a **magnitude**: direction lives in [`Side`], and signed
/// exposure is derived in a position, never stored here. The unsigned inner type
/// makes a negative quantity unrepresentable by construction.
///
/// [`Side`]: crate::Side
///
/// ```
/// use oath_model::Quantity;
/// let q = Quantity::from_raw(100);
/// assert_eq!(q.raw(), 100);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Quantity(u128);

impl Quantity {
    /// Wraps a raw scaled integer as a `Quantity`.
    #[must_use]
    pub const fn from_raw(raw: u128) -> Self {
        Self(raw)
    }

    /// Returns the raw scaled integer.
    #[must_use]
    pub const fn raw(self) -> u128 {
        self.0
    }

    /// Adds two quantities, returning an error on overflow instead of wrapping.
    ///
    /// # Errors
    ///
    /// Returns [`ArithmeticError::Overflow`] if the result exceeds `u128::MAX`.
    pub const fn checked_add(self, rhs: Self) -> Result<Self, ArithmeticError> {
        match self.0.checked_add(rhs.0) {
            Some(v) => Ok(Self(v)),
            None => Err(ArithmeticError::Overflow),
        }
    }

    /// Subtracts one quantity from another.
    ///
    /// # Errors
    ///
    /// Returns [`ArithmeticError::Underflow`] if `rhs` is greater than `self`
    /// (a quantity is a magnitude and cannot go negative).
    pub const fn checked_sub(self, rhs: Self) -> Result<Self, ArithmeticError> {
        match self.0.checked_sub(rhs.0) {
            Some(v) => Ok(Self(v)),
            None => Err(ArithmeticError::Underflow),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Quantity;
    use crate::error::ArithmeticError;
    use proptest::prelude::*;

    #[test]
    fn add_and_sub_happy_path() {
        assert_eq!(
            Quantity::from_raw(10).checked_add(Quantity::from_raw(5)),
            Ok(Quantity::from_raw(15))
        );
        assert_eq!(
            Quantity::from_raw(10).checked_sub(Quantity::from_raw(5)),
            Ok(Quantity::from_raw(5))
        );
    }

    #[test]
    fn add_overflow() {
        assert_eq!(
            Quantity::from_raw(u128::MAX).checked_add(Quantity::from_raw(1)),
            Err(ArithmeticError::Overflow)
        );
    }

    #[test]
    fn sub_underflow() {
        assert_eq!(
            Quantity::from_raw(0).checked_sub(Quantity::from_raw(1)),
            Err(ArithmeticError::Underflow)
        );
    }

    proptest! {
        #[test]
        fn raw_round_trip(x in any::<u128>()) {
            prop_assert_eq!(Quantity::from_raw(x).raw(), x);
        }

        #[test]
        fn sub_underflows_iff_rhs_greater(a in any::<u128>(), b in any::<u128>()) {
            let result = Quantity::from_raw(a).checked_sub(Quantity::from_raw(b));
            if b > a {
                prop_assert_eq!(result, Err(ArithmeticError::Underflow));
            } else {
                prop_assert_eq!(result, Ok(Quantity::from_raw(a - b)));
            }
        }

        #[test]
        fn json_round_trip(x in any::<u128>()) {
            let q = Quantity::from_raw(x);
            let back = serde_json::to_string(&q)
                .ok()
                .and_then(|s| serde_json::from_str::<Quantity>(&s).ok());
            prop_assert_eq!(back, Some(q));
        }
    }
}
