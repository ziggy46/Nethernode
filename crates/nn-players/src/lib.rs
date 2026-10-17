//! Player management: reaching a server through its management protocol
//! (1.21.9+) or RCON, Minecraft's own player lists on disk, and player data.

pub mod commands;
pub mod lists;
pub mod mgmt;
pub mod mojang;
pub mod nbt;
pub mod playerdata;
pub mod profile;
pub mod rcon;
pub mod snbt;

pub use mgmt::{MgmtClient, MgmtError, MgmtStream};
pub use profile::{Profile, offline_uuid, valid_name};
pub use rcon::{Rcon, RconError};
