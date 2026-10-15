//! The link between the NetherNode panel and an agent on another machine.
//!
//! ```text
//! WebSocket frames (any proxy in between may terminate TLS)
//!   └─ Noise XX: end-to-end encrypted, both static keys checked
//!        └─ Hello / Welcome (who the agent is, or its join code)
//!             └─ yamux streams (control, and more as features need them)
//! ```
//!
//! The agent always dials the panel. It knows the panel's public key from
//! its join token, so a proxy or impostor in the middle can't read or
//! answer for the panel; the panel knows each joined agent by its public
//! key. Nothing here knows about HTTP or WebSocket libraries: callers adapt
//! their socket to [`Frames`].

mod frames;
pub mod messages;
mod noise;
mod session;
mod token;

pub use frames::{FrameTransport, Frames};
pub use noise::{Handshaken, Keypair, LinkError, accept, connect};
pub use session::{Inbound, LinkStream, Session};
pub use token::{JoinToken, hash_secret};

/// The link protocol version, sent in [`messages::Hello`].
pub const PROTOCOL: u32 = 1;
