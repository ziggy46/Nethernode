//! Arbitrary JSON where the panel and an agent expect each other's
//! messages. A compromised agent (or panel) controls these bytes, so
//! decoding must fail cleanly, and what decodes must encode and decode again
//! (paths are checked as they decode).
//! Run: `cargo +nightly fuzz run protocol` from `crates/nn-node`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use nn_node::protocol::{AgentEvent, FileHeader, FolderReply, LogLine, Reply, Request};
use nn_node::{Archive, Restored};

fn decode<T: serde::de::DeserializeOwned>(data: &[u8]) -> Option<T> {
    serde_json::from_slice(data).ok()
}

fuzz_target!(|data: &[u8]| {
    // From the panel, on an agent.
    if let Some(request) = decode::<Request>(data) {
        // Paths are checked as they decode, so what re-encodes decodes again.
        let again = serde_json::to_vec(&request).unwrap();
        assert!(decode::<Request>(&again).is_some(), "a request didn't survive re-encoding");
    }
    // From an agent, on the panel.
    if let Some(Err(e)) = decode::<Reply>(data) {
        let _ = nn_node::NodeError::Remote(e).to_string();
    }
    if let Some(Err(failure)) = decode::<FolderReply>(data) {
        let _ = nn_files::FileError::from(failure).to_string();
    }
    let _ = decode::<AgentEvent>(data);
    let _ = decode::<LogLine>(data);
    let _ = decode::<FileHeader>(data);
    let _ = decode::<nn_store::Server>(data);
    let _ = decode::<Archive>(data);
    let _ = decode::<Restored>(data);
    let _ = decode::<Vec<nn_files::Entry>>(data);
});
