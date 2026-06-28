//! Root domain contract for OATH: the exact-domain numeric primitives
//! (`Price`, `Quantity`, `Side`) and the `ArithmeticError` their checked
//! operations return.
#![forbid(unsafe_code)]

mod error;
mod price;
mod quantity;
mod side;

pub use error::ArithmeticError;
pub use price::Price;
pub use quantity::Quantity;
pub use side::Side;
