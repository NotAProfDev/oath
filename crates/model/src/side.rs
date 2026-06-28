//! Order `Side` — the single source of truth for direction.

use serde::{Deserialize, Serialize};

/// The direction of an order or position: buy or sell.
///
/// `Side` is the single source of truth for direction; `Quantity` stays a
/// magnitude and never encodes sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// A buy: increases a long position.
    Buy,
    /// A sell: increases a short position.
    Sell,
}

impl Side {
    /// Returns the opposite side.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Side;
    use proptest::prelude::*;

    #[test]
    fn opposite_maps_buy_and_sell() {
        assert_eq!(Side::Buy.opposite(), Side::Sell);
        assert_eq!(Side::Sell.opposite(), Side::Buy);
    }

    #[test]
    fn serializes_to_expected_shape() {
        assert_eq!(
            serde_json::to_string(&Side::Buy).ok(),
            Some("\"Buy\"".to_owned())
        );
        assert_eq!(
            serde_json::to_string(&Side::Sell).ok(),
            Some("\"Sell\"".to_owned())
        );
    }

    proptest! {
        #[test]
        fn opposite_is_an_involution(s in prop_oneof![Just(Side::Buy), Just(Side::Sell)]) {
            prop_assert_eq!(s.opposite().opposite(), s);
        }
    }
}
