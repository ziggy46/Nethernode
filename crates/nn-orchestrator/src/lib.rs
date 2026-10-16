//! Docker lifecycle management for Minecraft server containers.
//!
//! [`Orchestrator`] creates, starts, stops, and deletes one container per
//! server (image `itzg/minecraft-server` by default), tracks each server's
//! state, streams stats and logs, adopts running containers after a
//! NetherNode restart, sleeps empty servers, and restarts crashed ones.

mod error;
pub mod host_path;
pub mod java;
mod lifecycle;
mod monitor;
pub mod ports;
pub mod properties;
pub mod ring;
pub mod runtime;
pub mod self_update;
pub mod spec;
mod watcher;

pub use error::{OrchestratorError, Result};
pub use lifecycle::{
    BeforeStart, CreateServer, HostInfo, Orchestrator, OrchestratorConfig, slugify,
};
pub use runtime::{Players, StatSample};
pub use watcher::{reconcile, spawn_background};
