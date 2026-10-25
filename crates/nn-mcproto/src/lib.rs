//! Minecraft Java Edition protocol primitives for the Smart Proxy.
//!
//! Covers only what a hostname-routing proxy needs before login completes:
//! VarInts, bounded frames, the Handshake, Login Start, login Disconnect,
//! the status (server list ping) exchange, and the legacy pre-1.7 ping.
//! Nothing here parses traffic after login, which may be encrypted. The
//! [`bedrock`] module is the exception to "Java": Bedrock's server list ping
//! over UDP, for the proxy's Bedrock listeners.
//!
//! Primary reference: <https://minecraft.wiki/w/Java_Edition_protocol>

pub mod bedrock;
#[cfg(feature = "tokio")]
pub mod client;
pub mod codec;
pub mod error;
pub mod frame;
pub mod handshake;
#[cfg(feature = "tokio")]
pub mod io;
pub mod legacy;
pub mod login;
pub mod status;
pub mod varint;

pub use error::{DecodeError, Result};
pub use frame::{FrameRange, parse_frame};
pub use handshake::{Handshake, NextState, normalize_hostname};
pub use legacy::{LegacyDetection, LegacyPing};
pub use login::LoginStart;
pub use status::StatusResponse;

/// Frame size limit for the Handshake (its fields need well under 1 KiB).
pub const MAX_HANDSHAKE_FRAME: usize = 2 * 1024;
/// Frame size limit for Login Start (1.19's signature data needs up to ~4.7 KiB).
pub const MAX_LOGIN_START_FRAME: usize = 16 * 1024;
/// Frame size limit for Status Request and Ping.
pub const MAX_STATUS_FRAME: usize = 64;
/// Frame size limit when reading a Status Response from a backend.
pub const MAX_STATUS_RESPONSE_FRAME: usize = 32_767 * 3 + 8;
