//! Root domain contract for OATH: the exact-domain numeric primitives
//! (`Price`, `Quantity`, `Side`) and the `ArithmeticError` their checked
//! operations return.
#![forbid(unsafe_code)]

mod error;

pub use error::ArithmeticError;
