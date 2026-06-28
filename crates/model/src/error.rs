//! Error type for checked exact-domain arithmetic.

use thiserror::Error;

/// An error from a checked exact-domain arithmetic operation on a `Price` or
/// `Quantity`.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArithmeticError {
    /// The result exceeds the maximum representable value.
    #[error("arithmetic overflow: result exceeds the representable maximum")]
    Overflow,
    /// The result is below the minimum representable value (for example,
    /// subtracting a larger magnitude from a smaller `Quantity`).
    #[error("arithmetic underflow: result is below the representable minimum")]
    Underflow,
}

#[cfg(test)]
mod tests {
    use super::ArithmeticError;

    #[test]
    fn variants_are_distinct() {
        assert_ne!(ArithmeticError::Overflow, ArithmeticError::Underflow);
        assert_ne!(
            ArithmeticError::Overflow.to_string(),
            ArithmeticError::Underflow.to_string()
        );
    }
}
