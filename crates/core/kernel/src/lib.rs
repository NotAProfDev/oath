//! The Kernel: the single-writer loop that owns canonical state and runs
//! Policies over a read-only view of it. Generic over `<R, E, P>`.
#![forbid(unsafe_code)]
