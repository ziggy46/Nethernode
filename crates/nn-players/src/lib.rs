//! Player management over RCON and the server management protocol.
//!
//! Phase 3 adds the RCON client (used for graceful stops); player actions
//! build on it in Phase 5.

pub mod rcon;

pub use rcon::{Rcon, RconError};
