//! Track deck transport, hotcues, loops and source positions.
//!
//! Status: **scaffold**. No behaviour is implemented yet; the crate exists so that
//! the module boundary, ownership and dependency direction are fixed before code
//! lands (`docs/architecture/crate-map.md`).
//!
//! - Backlog task: A03
//! - Requirements: DJP-DECK-001, DJP-DECK-002, DJP-DECK-003, DJP-DECK-004
//! - Source frames and the deck clock are different timebases (DJP-DATA-002).
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
