//! Resumable chunked uploads with the tus 1.0 protocol (core, creation,
//! termination, checksum, and expiration extensions).
//!
//! Large files reach NetherNode in chunks small enough for Cloudflare's
//! request limits, and an interrupted upload resumes from the last byte that
//! reached the disk. This crate holds the protocol parsing and the partial
//! files; the HTTP endpoints and bookkeeping live in `nn-api`.

pub mod protocol;
pub mod temp;

pub use temp::{Chunk, ChunkError, TempStore, available_space, sha256_file, write_chunk};
