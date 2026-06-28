//! Root domain contract for OATH: the exact-domain numeric primitives
//! (`Price`, `Quantity`, `Side`) and the `ArithmeticError` their checked
//! operations return.
#![forbid(unsafe_code)]

mod error;
mod side;

pub use error::ArithmeticError;
pub use side::Side;
