//! Root domain contract for OATH: the exact-domain numeric primitives
//! (`Price`, `Quantity`, `Side`) and the `ArithmeticError` their checked
//! operations return.
#![forbid(unsafe_code)]

mod error;
mod quantity;
mod side;

pub use error::ArithmeticError;
pub use quantity::Quantity;
pub use side::Side;
