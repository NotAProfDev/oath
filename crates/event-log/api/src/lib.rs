//! The Event Log and Snapshot traits: the append-only, totally-ordered record
//! Core's state is a pure fold over, plus point-in-time recovery captures.
#![forbid(unsafe_code)]
