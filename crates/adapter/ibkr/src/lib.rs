//! IBKR venue adapter.
//!
//! Surface-neutral by design: the Client Portal API v1 wire layer lives under
//! [`cpapi`]. Future `webapi` (beta OAuth 2.0) and `tws` (socket) surfaces will be
//! siblings. This crate is the venue-side half of the ADR-0003 anti-corruption
//! boundary — it faithfully mirrors IBKR's wire and performs no translation to
//! OATH domain types (deferred until those types exist).
#![forbid(unsafe_code)]

pub mod cpapi;
